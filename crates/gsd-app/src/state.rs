//! Shared application state handed to every request handler.

use std::{fmt, sync::Arc};

use gsd_core::{
    audit::AuditLog,
    auth::{AuthError, AuthService},
    random::random_token,
    store::SqlitePool,
};

use crate::config::Config;

/// Everything handlers need. Cheap to clone (handles and `Arc`s only).
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub auth: AuthService,
    pub audit: AuditLog,
    pub setup_gate: SetupGate,
}

impl AppState {
    /// Wires the services together. Also decides whether first-run setup needs a token.
    pub async fn build(config: Config, pool: SqlitePool) -> Result<Self, AuthError> {
        let auth = AuthService::new(pool.clone(), config.session_ttl);
        let setup_gate = SetupGate::for_startup(&config, &auth).await?;
        Ok(Self {
            config: Arc::new(config),
            auth,
            audit: AuditLog::new(pool),
            setup_gate,
        })
    }
}

/// Protects first-run setup when the server is reachable from other machines.
///
/// If no owner exists yet and the app listens on a non-loopback address, anyone on the network
/// could claim the owner account. In that case a one-time token is printed to the server log
/// and the setup form must include it.
#[derive(Clone, Default)]
pub struct SetupGate {
    token: Option<Arc<str>>,
}

impl SetupGate {
    /// No token needed.
    pub fn open() -> Self {
        Self::default()
    }

    pub fn with_token(token: impl Into<Arc<str>>) -> Self {
        Self {
            token: Some(token.into()),
        }
    }

    async fn for_startup(config: &Config, auth: &AuthService) -> Result<Self, AuthError> {
        if config.bind.ip().is_loopback() || !auth.is_setup_required().await? {
            return Ok(Self::open());
        }
        Ok(Self::with_token(random_token::<16>()?))
    }

    pub fn requires_token(&self) -> bool {
        self.token.is_some()
    }

    /// The token, for printing to the log at startup.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Whether `provided` satisfies the gate (constant-time comparison).
    pub fn allows(&self, provided: Option<&str>) -> bool {
        match (&self.token, provided) {
            (None, _) => true,
            (Some(expected), Some(given)) => constant_time_eq(expected.as_bytes(), given.trim().as_bytes()),
            (Some(_), None) => false,
        }
    }
}

impl fmt::Debug for SetupGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupGate")
            .field("requires_token", &self.requires_token())
            .finish()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_gate_allows_everything() {
        assert!(SetupGate::open().allows(None));
    }

    #[test]
    fn token_gate_checks_the_token() {
        let gate = SetupGate::with_token("secret-token");
        assert!(gate.allows(Some("secret-token")));
        assert!(gate.allows(Some(" secret-token ")));
        assert!(!gate.allows(Some("wrong")));
        assert!(!gate.allows(None));
    }
}
