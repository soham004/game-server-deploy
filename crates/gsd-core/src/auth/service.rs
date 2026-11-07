//! The auth service: first-run setup, login and logout, session authentication, password changes.

use std::{fmt, net::IpAddr, sync::OnceLock};

use chrono::{TimeDelta, Utc};
use sqlx::SqlitePool;

use crate::{
    auth::{
        Actor, AuthError, GlobalRole, User, Username,
        grants::GrantRepo,
        password as passwords,
        sessions::{self, NewSession, SessionRepo},
        users::{NewUser, UserRepo},
    },
    random::random_token,
};

/// Sessions are refreshed (sliding expiry) at most this often, to avoid a write per request.
const SESSION_TOUCH_INTERVAL: TimeDelta = TimeDelta::seconds(60);

/// Where a request came from; recorded on the session for the "active sessions" list.
#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
}

/// A successful sign-in: the user and the secret session token for the cookie.
pub struct LoginOutcome {
    pub user: User,
    pub session_token: String,
}

impl fmt::Debug for LoginOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginOutcome")
            .field("user", &self.user)
            .field("session_token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AuthService {
    users: UserRepo,
    sessions: SessionRepo,
    grants: GrantRepo,
    session_ttl: TimeDelta,
}

impl AuthService {
    pub fn new(pool: SqlitePool, session_ttl: TimeDelta) -> Self {
        Self {
            users: UserRepo::new(pool.clone()),
            sessions: SessionRepo::new(pool.clone()),
            grants: GrantRepo::new(pool),
            session_ttl,
        }
    }

    pub fn grants(&self) -> &GrantRepo {
        &self.grants
    }

    pub fn session_ttl(&self) -> TimeDelta {
        self.session_ttl
    }

    /// True until the first owner account exists.
    pub async fn is_setup_required(&self) -> Result<bool, AuthError> {
        Ok(self.users.count().await? == 0)
    }

    /// Creates the first owner and signs them in. Fails with `AlreadySetUp` once any user exists.
    pub async fn create_owner(
        &self,
        username: &str,
        password: &str,
        client: &ClientInfo,
    ) -> Result<LoginOutcome, AuthError> {
        // Report "already set up" before judging the input; the insert below still guards races.
        if !self.is_setup_required().await? {
            return Err(AuthError::AlreadySetUp);
        }
        let username = Username::parse(username)?;
        passwords::validate_new(password)?;
        let new_user = NewUser {
            username,
            password_hash: hash_in_background(password).await?,
            role: GlobalRole::Owner,
            must_change_password: false,
        };

        let owner = self
            .users
            .insert_if_no_users(&new_user)
            .await?
            .ok_or(AuthError::AlreadySetUp)?;
        self.start_session(owner, client).await
    }

    /// Checks credentials and starts a session.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        client: &ClientInfo,
    ) -> Result<LoginOutcome, AuthError> {
        let Some(stored) = self.users.find_by_username(username.trim()).await? else {
            // WHY: spend the same time as a real check, so timing doesn't reveal which usernames exist.
            let _ = verify_in_background(password, dummy_hash()).await;
            return Err(AuthError::InvalidCredentials);
        };

        let password_matches = verify_in_background(password, &stored.password_hash).await?;
        if !password_matches || stored.user.disabled {
            return Err(AuthError::InvalidCredentials);
        }
        self.start_session(stored.user, client).await
    }

    /// Resolves a session token into an [`Actor`], extending the session if it is in use.
    pub async fn authenticate(&self, session_token: &str) -> Result<Actor, AuthError> {
        let now = Utc::now();
        let token_hash = sessions::hash_token(session_token);

        let session = self
            .sessions
            .find_active(&token_hash, now)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        let stored = self
            .users
            .find_by_id(session.user_id)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        if stored.user.disabled {
            return Err(AuthError::SessionInvalid);
        }

        if now - session.last_seen_at > SESSION_TOUCH_INTERVAL {
            self.sessions
                .extend(&token_hash, now, now + self.session_ttl)
                .await?;
        }

        let grants = self.grants.for_user(stored.user.id).await?;
        Ok(Actor::new(stored.user, grants))
    }

    pub async fn logout(&self, session_token: &str) -> Result<(), AuthError> {
        self.sessions.delete(&sessions::hash_token(session_token)).await?;
        Ok(())
    }

    /// Changes the actor's own password, signs out all their sessions, and starts a fresh one.
    pub async fn change_password(
        &self,
        actor: &Actor,
        current_password: &str,
        new_password: &str,
        client: &ClientInfo,
    ) -> Result<LoginOutcome, AuthError> {
        passwords::validate_new(new_password)?;
        let stored = self
            .users
            .find_by_id(actor.id())
            .await?
            .ok_or(AuthError::UserNotFound)?;
        if !verify_in_background(current_password, &stored.password_hash).await? {
            return Err(AuthError::InvalidCredentials);
        }

        let new_hash = hash_in_background(new_password).await?;
        self.users.update_password(actor.id(), &new_hash, false).await?;
        self.sessions.delete_for_user(actor.id()).await?;

        let mut user = stored.user;
        user.must_change_password = false;
        self.start_session(user, client).await
    }

    /// Recovery (CLI): sets a random temporary password that must be changed at next sign-in,
    /// and signs the user out everywhere. Returns the temporary password.
    pub async fn reset_password(&self, username: &str) -> Result<String, AuthError> {
        let stored = self
            .users
            .find_by_username(username.trim())
            .await?
            .ok_or(AuthError::UserNotFound)?;
        let temporary = random_token::<12>()?;
        let new_hash = hash_in_background(&temporary).await?;

        self.users
            .update_password(stored.user.id, &new_hash, true)
            .await?;
        self.sessions.delete_for_user(stored.user.id).await?;
        Ok(temporary)
    }

    pub async fn purge_expired_sessions(&self) -> Result<u64, AuthError> {
        Ok(self.sessions.delete_expired(Utc::now()).await?)
    }

    async fn start_session(&self, user: User, client: &ClientInfo) -> Result<LoginOutcome, AuthError> {
        let session_token = random_token::<32>()?;
        let now = Utc::now();
        let new_session = NewSession {
            token_hash: &sessions::hash_token(&session_token),
            user_id: user.id,
            now,
            expires_at: now + self.session_ttl,
            user_agent: client.user_agent.as_deref(),
            ip: client.ip.map(|ip| ip.to_string()),
        };
        self.sessions.insert(&new_session).await?;
        Ok(LoginOutcome { user, session_token })
    }
}

async fn hash_in_background(password: &str) -> Result<String, AuthError> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || passwords::hash(&password))
        .await
        .map_err(|_| AuthError::BackgroundTask)?
        .map_err(AuthError::from)
}

async fn verify_in_background(password: &str, stored_hash: &str) -> Result<bool, AuthError> {
    let (password, stored_hash) = (password.to_owned(), stored_hash.to_owned());
    tokio::task::spawn_blocking(move || passwords::verify(&password, &stored_hash))
        .await
        .map_err(|_| AuthError::BackgroundTask)?
        .map_err(AuthError::from)
}

/// A real Argon2 hash used to equalise timing for unknown usernames.
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| passwords::hash("timing-equaliser").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::{Grant, Permission},
        ids::InstanceId,
        store,
    };

    const PASSWORD: &str = "correct horse battery";

    async fn service() -> AuthService {
        AuthService::new(store::open_in_memory().await.unwrap(), TimeDelta::days(30))
    }

    #[tokio::test]
    async fn setup_happens_exactly_once() {
        let auth = service().await;
        assert!(auth.is_setup_required().await.unwrap());

        let outcome = auth
            .create_owner("owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();
        assert_eq!(outcome.user.role, GlobalRole::Owner);
        assert!(!auth.is_setup_required().await.unwrap());

        let again = auth.create_owner("other", PASSWORD, &ClientInfo::default()).await;
        assert!(matches!(again, Err(AuthError::AlreadySetUp)));
    }

    #[tokio::test]
    async fn login_authenticate_logout() {
        let auth = service().await;
        auth.create_owner("Owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();

        let wrong = auth
            .login("owner", "not the password", &ClientInfo::default())
            .await;
        assert!(matches!(wrong, Err(AuthError::InvalidCredentials)));
        let unknown = auth.login("nobody", PASSWORD, &ClientInfo::default()).await;
        assert!(matches!(unknown, Err(AuthError::InvalidCredentials)));

        // Usernames are case-insensitive.
        let session = auth
            .login("OWNER", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();
        let actor = auth.authenticate(&session.session_token).await.unwrap();
        assert_eq!(actor.user().username.as_str(), "Owner");

        auth.logout(&session.session_token).await.unwrap();
        assert!(matches!(
            auth.authenticate(&session.session_token).await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn authenticate_loads_member_grants() {
        let auth = service().await;
        auth.create_owner("owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();
        let member = auth
            .users
            .insert(&NewUser {
                username: Username::parse("friend").unwrap(),
                password_hash: passwords::hash(PASSWORD).unwrap(),
                role: GlobalRole::Member,
                must_change_password: false,
            })
            .await
            .unwrap();
        let instance = InstanceId::generate();
        auth.grants()
            .set(member.id, instance, Grant::Moderator)
            .await
            .unwrap();

        let session = auth
            .login("friend", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();
        let actor = auth.authenticate(&session.session_token).await.unwrap();
        assert!(actor.can(Permission::PlayersManage, instance));
        assert!(!actor.can(Permission::ConsoleWrite, instance));
    }

    #[tokio::test]
    async fn changing_password_revokes_old_sessions() {
        let auth = service().await;
        let first = auth
            .create_owner("owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();
        let actor = auth.authenticate(&first.session_token).await.unwrap();

        let fresh = auth
            .change_password(&actor, PASSWORD, "a brand new passphrase", &ClientInfo::default())
            .await
            .unwrap();

        assert!(auth.authenticate(&first.session_token).await.is_err());
        assert!(auth.authenticate(&fresh.session_token).await.is_ok());
        assert!(
            auth.login("owner", "a brand new passphrase", &ClientInfo::default())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn reset_password_forces_a_change() {
        let auth = service().await;
        auth.create_owner("owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap();

        let temporary = auth.reset_password("owner").await.unwrap();
        let session = auth
            .login("owner", &temporary, &ClientInfo::default())
            .await
            .unwrap();
        assert!(session.user.must_change_password);
    }
}
