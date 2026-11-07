//! Shared application state handed to every request handler, and how it is wired together.

use std::{collections::HashMap, fmt, sync::Arc};

use gsd_core::{
    audit::AuditLog,
    auth::{AuthError, AuthService},
    events::EventBus,
    game::GameAdapter,
    instances::{AllocationPolicy, InstanceRegistry, InstanceService, registry::RegistryError},
    java::JavaRuntimes,
    jobs::JobRunner,
    net::http_client,
    random::random_token,
    store::SqlitePool,
    supervisor::{SandboxSupport, Supervisors},
};
use gsd_minecraft::{Catalog, Endpoints, GAME_ID, MinecraftAdapter, MinecraftService};
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// Everything handlers need. Cheap to clone (handles and `Arc`s only).
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub auth: AuthService,
    pub audit: AuditLog,
    pub setup_gate: SetupGate,
    pub events: EventBus,
    pub jobs: JobRunner,
    pub java: JavaRuntimes,
    pub instances: InstanceService,
    pub minecraft: MinecraftService,
    /// Cancelled when the app shuts down (closes live connections so shutdown isn't held open).
    pub shutdown: CancellationToken,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("could not create the HTTP client")]
    Http(#[from] reqwest::Error),
}

/// Overrides for tests (mock upstream APIs, no port probing, no sandbox).
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub endpoints: Option<Endpoints>,
    pub adoptium_api: Option<String>,
    pub sandbox: Option<SandboxSupport>,
    pub probe_ports: Option<bool>,
}

impl AppState {
    pub async fn build(config: Config, pool: SqlitePool) -> Result<Self, BuildError> {
        Self::build_with(config, pool, BuildOptions::default()).await
    }

    /// Wires the services together. Also decides whether first-run setup needs a token.
    pub async fn build_with(
        config: Config,
        pool: SqlitePool,
        options: BuildOptions,
    ) -> Result<Self, BuildError> {
        let auth = AuthService::new(pool.clone(), config.session_ttl);
        let setup_gate = SetupGate::for_startup(&config, &auth).await?;
        let events = EventBus::new();
        let jobs = JobRunner::new(events.clone());
        let client = http_client()?;

        let java = match options.adoptium_api {
            Some(api) => JavaRuntimes::with_api(config.runtimes_dir(), client.clone(), api, false),
            None => JavaRuntimes::new(config.runtimes_dir(), client.clone()),
        };
        let policy = AllocationPolicy {
            game_ports: config.game_ports,
            sandbox_uids: AllocationPolicy::DEFAULT_SANDBOX_UIDS,
            probe_ports: options.probe_ports.unwrap_or(true),
        };
        let (registry, problems) =
            InstanceRegistry::load(config.instances_dir(), config.trash_dir(), policy, events.clone())?;
        for problem in problems {
            tracing::error!(path = %problem.path.display(), error = %problem.error, "skipping an instance that could not be loaded");
        }

        let support = options.sandbox.unwrap_or_else(SandboxSupport::detect);
        let supervisors = Supervisors::new(events.clone(), support);
        let mut adapters: HashMap<String, Arc<dyn GameAdapter>> = HashMap::new();
        adapters.insert(GAME_ID.to_owned(), Arc::new(MinecraftAdapter));
        let instances = InstanceService::new(
            registry.clone(),
            supervisors,
            java.clone(),
            jobs.clone(),
            adapters,
        );

        let catalog = Catalog::new(
            client,
            options.endpoints.unwrap_or_default(),
            config.catalog_cache_dir(),
        );
        let minecraft = MinecraftService::new(
            catalog,
            registry,
            java.clone(),
            jobs.clone(),
            config.download_cache_dir(),
        );

        Ok(Self {
            config: Arc::new(config),
            auth,
            audit: AuditLog::new(pool),
            setup_gate,
            events,
            jobs,
            java,
            instances,
            minecraft,
            shutdown: CancellationToken::new(),
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
