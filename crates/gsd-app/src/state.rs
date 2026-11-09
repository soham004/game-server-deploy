//! Shared application state handed to every request handler, and how it is wired together.

use std::{collections::HashMap, fmt, sync::Arc};

use gsd_core::settings::{AppSettings, CURSEFORGE_API_KEY};
use gsd_core::{
    audit::AuditLog,
    auth::{AuthError, AuthService},
    backups::BackupService,
    events::EventBus,
    game::GameAdapter,
    instances::{AllocationPolicy, InstanceRegistry, InstanceService, registry::RegistryError},
    java::JavaRuntimes,
    jobs::JobRunner,
    net::http_client,
    random::random_token,
    scheduler::Scheduler,
    store::SqlitePool,
    supervisor::{
        HostSandbox, SandboxSupport, Supervisors,
        sandbox::{cgroup::CgroupTree, check},
    },
};
use gsd_minecraft::{Catalog, Endpoints, GAME_ID, MinecraftAdapter, MinecraftPaths, MinecraftService};
use gsd_mods::{ContentService, SourceApis, Sources};
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
    pub backups: BackupService,
    pub scheduler: Scheduler,
    pub minecraft: MinecraftService,
    pub content: ContentService,
    pub settings: AppSettings,
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
    /// Skips host detection and the data-dir sandbox check.
    pub sandbox: Option<HostSandbox>,
    pub probe_ports: Option<bool>,
    /// How long a backup waits for a running server to confirm its save.
    pub backup_save_timeout: Option<std::time::Duration>,
    /// Where the content sources' APIs are (mocks in tests).
    pub source_apis: Option<SourceApis>,
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

        let host = match options.sandbox {
            Some(host) => host,
            None => detect_host_sandbox(&config).await,
        };
        let supervisors = Supervisors::new(events.clone(), host.clone());
        let mut adapters: HashMap<String, Arc<dyn GameAdapter>> = HashMap::new();
        adapters.insert(GAME_ID.to_owned(), Arc::new(MinecraftAdapter));
        let instances = InstanceService::new(
            registry.clone(),
            supervisors,
            java.clone(),
            jobs.clone(),
            adapters,
        );

        let mut backups = BackupService::new(
            pool.clone(),
            instances.clone(),
            jobs.clone(),
            events.clone(),
            config.backups_dir(),
        );
        if let Some(timeout) = options.backup_save_timeout {
            backups = backups.with_save_timeout(timeout);
        }
        let scheduler = Scheduler::new(
            pool.clone(),
            instances.clone(),
            backups.clone(),
            auth.clone(),
            jobs.clone(),
            events.clone(),
        );

        let content = ContentService::new(
            Sources::new(client.clone(), source_apis(&config, options.source_apis)),
            instances.clone(),
            jobs.clone(),
            events.clone(),
            config.content_cache_dir(),
        );
        let settings = AppSettings::new(pool.clone());
        load_curseforge_key(&settings, &content).await;

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
            host,
            MinecraftPaths {
                download_cache: config.download_cache_dir(),
                imports: config.imports_dir(),
            },
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
            backups,
            scheduler,
            minecraft,
            content,
            settings,
            shutdown: CancellationToken::new(),
        })
    }
}

/// Where the content sources are: what a test says, else the real ones (with the configured
/// `CurseForge`, if any).
fn source_apis(config: &Config, chosen: Option<SourceApis>) -> SourceApis {
    chosen.unwrap_or_else(|| {
        let mut apis = SourceApis::default();
        if let Some(curseforge) = &config.curseforge_api {
            apis.curseforge.clone_from(curseforge);
        }
        apis
    })
}

/// Starts using the saved `CurseForge` key. A key that can't be used leaves `CurseForge` off, and the
/// app running: it is one optional source.
async fn load_curseforge_key(settings: &AppSettings, content: &ContentService) {
    let key = match settings.get(CURSEFORGE_API_KEY).await {
        Ok(key) => key,
        Err(error) => {
            tracing::error!(%error, "could not read the CurseForge key; CurseForge stays off");
            return;
        }
    };
    if let Err(error) = content.set_curseforge_key(key.as_deref()) {
        tracing::error!(%error, "the saved CurseForge key can't be used; CurseForge stays off");
    }
}

/// The host's isolation layers, the result of the data-directory check below, and hard limits
/// (control groups) when this host delegates them to the app and a probe proves they work.
async fn detect_host_sandbox(config: &Config) -> HostSandbox {
    let support = SandboxSupport::detect();
    // Before any probe runs: setting up may move the app into a subgroup of its own, which is
    // refused while other processes (such as a probe) share its group.
    let cgroups = CgroupTree::detect();
    let host =
        HostSandbox::new(support, check_sandbox_in_data_dir(config, support).await).with_cgroups(cgroups);
    match std::env::current_exe() {
        Ok(probe) => check::verify_cgroups(&probe, &config.sandbox_check_dir(), host).await,
        Err(_) => host,
    }
}

/// Proves a sandboxed server could use its folder in the data directory. Returns why not, if not.
///
/// Without this, a data directory on WSL's Windows drives fails much later and cryptically: the
/// server process can't open its own jar and crash-loops.
async fn check_sandbox_in_data_dir(config: &Config, support: SandboxSupport) -> Option<String> {
    let result = async {
        let probe = std::env::current_exe()?;
        check::own_folder_usable(&probe, &config.sandbox_check_dir(), support).await
    }
    .await;
    match result {
        Ok(true) => None,
        Ok(false) => Some(format!(
            "Servers can't start: the sandbox can't enforce its rules on the filesystem holding the data \
             folder ({}). This happens on WSL's Windows drives (/mnt/c, /mnt/e, …) and on some network \
             and FUSE mounts. Move the data folder to a Linux filesystem, for example `--data-dir ~/{}-data`.",
            config.data_dir.display(),
            gsd_brand::SLUG
        )),
        Err(error) => {
            tracing::warn!(%error, "could not test the sandbox in the data folder");
            None
        }
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
