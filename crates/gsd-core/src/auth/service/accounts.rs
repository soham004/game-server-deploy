//! Managing accounts (owners only, see `users.manage`): list, create, change role, disable,
//! delete, reset passwords, sign out, and per-instance grants.
//!
//! Rules enforced here, not only in the UI: nobody changes their own role or account status,
//! and the last active owner can never be demoted, disabled or deleted (the app would be left
//! without anyone who can manage it).

use chrono::Utc;

use super::{AuthService, hash_in_background};
use crate::{
    auth::{Actor, AuthError, GlobalRole, Grant, User, Username, users::NewUser},
    ids::{InstanceId, UserId},
    random::random_token,
};

/// One row of the user list.
#[derive(Debug, Clone)]
pub struct UserSummary {
    pub user: User,
    pub active_sessions: i64,
}

/// What to change about an account; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountChange {
    pub role: Option<GlobalRole>,
    pub disabled: Option<bool>,
}

impl AuthService {
    pub async fn list_users(&self) -> Result<Vec<UserSummary>, AuthError> {
        let sessions = self.sessions.count_active_by_user(Utc::now()).await?;
        Ok(self
            .users
            .list()
            .await?
            .into_iter()
            .map(|user| UserSummary {
                active_sessions: sessions.get(&user.id).copied().unwrap_or(0),
                user,
            })
            .collect())
    }

    /// Creates an account with a random temporary password (returned once) that must be
    /// changed at first sign-in.
    pub async fn create_user(&self, username: &str, role: GlobalRole) -> Result<(User, String), AuthError> {
        let username = Username::parse(username)?;
        let temporary = random_token::<12>()?;
        let new = NewUser {
            username,
            password_hash: hash_in_background(&temporary).await?,
            role,
            must_change_password: true,
        };
        let user = self.users.insert(&new).await.map_err(|error| {
            if error.is_unique_violation() {
                AuthError::UsernameTaken
            } else {
                AuthError::Repo(error)
            }
        })?;
        Ok((user, temporary))
    }

    /// Changes another user's role and/or status. Disabling signs them out everywhere.
    pub async fn update_user(
        &self,
        actor: &Actor,
        id: UserId,
        change: &AccountChange,
    ) -> Result<User, AuthError> {
        if id == actor.id() && *change != AccountChange::default() {
            return Err(AuthError::OwnAccount);
        }
        let current = self.find(id).await?;
        let loses_owner = current.role == GlobalRole::Owner
            && !current.disabled
            && (change.role.is_some_and(|role| role != GlobalRole::Owner) || change.disabled == Some(true));
        if loses_owner {
            self.ensure_another_owner().await?;
        }
        if let Some(role) = change.role {
            self.users.update_role(id, role).await?;
        }
        if let Some(disabled) = change.disabled {
            self.users.set_disabled(id, disabled).await?;
            if disabled {
                self.sessions.delete_for_user(id).await?;
            }
        }
        self.find(id).await
    }

    /// Deletes another user, with their sessions and grants.
    pub async fn delete_user(&self, actor: &Actor, id: UserId) -> Result<(), AuthError> {
        if id == actor.id() {
            return Err(AuthError::OwnAccount);
        }
        let user = self.find(id).await?;
        if user.role == GlobalRole::Owner && !user.disabled {
            self.ensure_another_owner().await?;
        }
        Ok(self.users.delete(id).await?)
    }

    /// A new temporary password (returned once), to be changed at next sign-in; signs the user
    /// out everywhere.
    pub async fn reset_user_password(&self, id: UserId) -> Result<String, AuthError> {
        let user = self.find(id).await?;
        self.reset_password(user.username.as_str()).await
    }

    /// Ends every session of the user.
    pub async fn sign_out_everywhere(&self, id: UserId) -> Result<(), AuthError> {
        self.find(id).await?;
        Ok(self.sessions.delete_for_user(id).await?)
    }

    pub async fn user_grants(&self, id: UserId) -> Result<Vec<(InstanceId, Grant)>, AuthError> {
        self.find(id).await?;
        Ok(self.grants.list_for_user(id).await?)
    }

    async fn find(&self, id: UserId) -> Result<User, AuthError> {
        Ok(self
            .users
            .find_by_id(id)
            .await?
            .ok_or(AuthError::UserNotFound)?
            .user)
    }

    /// `Err(LastOwner)` unless at least two active owners exist (so one may go).
    async fn ensure_another_owner(&self) -> Result<(), AuthError> {
        if self.users.count_active_owners().await? < 2 {
            return Err(AuthError::LastOwner);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashMap;

    use chrono::TimeDelta;

    use super::*;
    use crate::{
        auth::{ClientInfo, Permission},
        store,
    };

    const PASSWORD: &str = "correct horse battery";

    /// A service with one owner; returns the owner as an actor.
    async fn with_owner() -> (AuthService, Actor) {
        let auth = AuthService::new(store::open_in_memory().await.unwrap(), TimeDelta::days(30));
        let owner = auth
            .create_owner("owner", PASSWORD, &ClientInfo::default())
            .await
            .unwrap()
            .user;
        (auth, Actor::new(owner, HashMap::new()))
    }

    #[tokio::test]
    async fn created_users_must_change_their_temporary_password() {
        let (auth, _) = with_owner().await;
        let (user, temporary) = auth.create_user("friend", GlobalRole::Member).await.unwrap();
        assert!(user.must_change_password);
        assert_eq!(temporary.len(), 16, "12 random bytes, base64");
        let login = auth
            .login("friend", &temporary, &ClientInfo::default())
            .await
            .unwrap();
        assert!(login.user.must_change_password);
        assert!(matches!(
            auth.create_user("FRIEND", GlobalRole::Member).await,
            Err(AuthError::UsernameTaken)
        ));
        assert!(matches!(
            auth.create_user("x", GlobalRole::Member).await,
            Err(AuthError::InvalidUsername(_))
        ));

        let names: Vec<String> = auth
            .list_users()
            .await
            .unwrap()
            .iter()
            .map(|s| s.user.username.to_string())
            .collect();
        assert_eq!(names, ["friend", "owner"]);
    }

    #[tokio::test]
    async fn the_last_owner_stays_an_owner() {
        let (auth, owner) = with_owner().await;
        let (second, _) = auth.create_user("second", GlobalRole::Admin).await.unwrap();
        let second_actor = Actor::new(second.clone(), HashMap::new());

        // Nobody changes their own account.
        let demote_self = AccountChange {
            role: Some(GlobalRole::Member),
            disabled: None,
        };
        assert!(matches!(
            auth.update_user(&owner, owner.id(), &demote_self).await,
            Err(AuthError::OwnAccount)
        ));
        assert!(matches!(
            auth.delete_user(&owner, owner.id()).await,
            Err(AuthError::OwnAccount)
        ));

        // The only owner can't be demoted, disabled or deleted by anyone.
        for change in [
            demote_self.clone(),
            AccountChange {
                role: None,
                disabled: Some(true),
            },
        ] {
            assert!(matches!(
                auth.update_user(&second_actor, owner.id(), &change).await,
                Err(AuthError::LastOwner)
            ));
        }
        assert!(matches!(
            auth.delete_user(&second_actor, owner.id()).await,
            Err(AuthError::LastOwner)
        ));

        // With a second owner, one may go.
        let promote = AccountChange {
            role: Some(GlobalRole::Owner),
            disabled: None,
        };
        auth.update_user(&owner, second.id, &promote).await.unwrap();
        let demoted = auth
            .update_user(&second_actor, owner.id(), &demote_self)
            .await
            .unwrap();
        assert_eq!(demoted.role, GlobalRole::Member);
    }

    #[tokio::test]
    async fn disabling_signs_out_and_blocks_sign_in() {
        let (auth, owner) = with_owner().await;
        let (user, temporary) = auth.create_user("friend", GlobalRole::Member).await.unwrap();
        let session = auth
            .login("friend", &temporary, &ClientInfo::default())
            .await
            .unwrap();
        assert_eq!(
            auth.list_users()
                .await
                .unwrap()
                .iter()
                .find(|s| s.user.id == user.id)
                .unwrap()
                .active_sessions,
            1
        );

        let disable = AccountChange {
            role: None,
            disabled: Some(true),
        };
        assert!(
            auth.update_user(&owner, user.id, &disable)
                .await
                .unwrap()
                .disabled
        );
        assert!(
            auth.authenticate(&session.session_token).await.is_err(),
            "signed out"
        );
        assert!(
            auth.login("friend", &temporary, &ClientInfo::default())
                .await
                .is_err(),
            "can't sign in"
        );

        let enable = AccountChange {
            role: None,
            disabled: Some(false),
        };
        auth.update_user(&owner, user.id, &enable).await.unwrap();
        assert!(
            auth.login("friend", &temporary, &ClientInfo::default())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn reset_sign_out_delete_and_grants() {
        let (auth, owner) = with_owner().await;
        let (user, old) = auth.create_user("friend", GlobalRole::Member).await.unwrap();
        let session = auth.login("friend", &old, &ClientInfo::default()).await.unwrap();

        let new = auth.reset_user_password(user.id).await.unwrap();
        assert_ne!(new, old);
        assert!(auth.authenticate(&session.session_token).await.is_err());
        assert!(auth.login("friend", &old, &ClientInfo::default()).await.is_err());
        let session = auth.login("friend", &new, &ClientInfo::default()).await.unwrap();
        auth.sign_out_everywhere(user.id).await.unwrap();
        assert!(auth.authenticate(&session.session_token).await.is_err());

        let instance = InstanceId::generate();
        auth.grants()
            .set(user.id, instance, Grant::Operator)
            .await
            .unwrap();
        assert_eq!(
            auth.user_grants(user.id).await.unwrap(),
            [(instance, Grant::Operator)]
        );
        let login = auth.login("friend", &new, &ClientInfo::default()).await.unwrap();
        let actor = auth.authenticate(&login.session_token).await.unwrap();
        assert!(actor.can(Permission::Lifecycle, instance));

        auth.delete_user(&owner, user.id).await.unwrap();
        assert!(matches!(
            auth.user_grants(user.id).await,
            Err(AuthError::UserNotFound)
        ));
        assert!(
            auth.authenticate(&login.session_token).await.is_err(),
            "sessions went with the user"
        );
        assert!(matches!(
            auth.reset_user_password(UserId::generate()).await,
            Err(AuthError::UserNotFound)
        ));
    }
}
