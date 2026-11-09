//! Creating Minecraft instances.
//!
//! `create` validates the request against the catalog *synchronously* (so typos fail at once),
//! registers the instance, then installs it in a background job: Java (if missing), the server
//! files (see `install`), ownership for the sandbox user, and the launch the install produced.
//! The instance can't be started until the job marks it installed; a failed install can be
//! retried.

use std::path::PathBuf;

use chrono::Utc;
use gsd_core::{
    ids::JobId,
    instances::{
        Instance, InstanceRegistry, NewInstance,
        definition::{Behavior, JavaSettings, JvmPreset, LaunchSpec, RuntimeChoice},
        registry::RegistryError,
    },
    java::JavaRuntimes,
    jobs::{JobContext, JobError, JobFailure, JobRunner, JobSpec},
    supervisor::HostSandbox,
};

use crate::{
    adapter::GAME_ID,
    catalog::{Catalog, CatalogError},
    install::{self, InstallTarget, SERVER_JAR},
    loader::LoaderKind,
    settings::{MinecraftServer, SettingsError},
};

use gsd_core::instances::settings::{MAX_MEMORY_MB, MAX_NAME_CHARS as MAX_NAME_LEN, MIN_MEMORY_MB};

/// JVM flags every Minecraft server gets: a plain console instead of `JLine`'s interactive prompt
/// (which garbles piped output), while keeping colour codes.
pub const DEFAULT_JVM_ARGS: [&str; 2] = ["-Dterminal.jline=false", "-Dterminal.ansi=true"];

/// The JVM arguments a user added: everything except [`DEFAULT_JVM_ARGS`] (which the console
/// needs, so the settings form neither shows nor can remove them).
pub fn user_jvm_args(all: &[String]) -> Vec<String> {
    all.iter()
        .filter(|arg| !DEFAULT_JVM_ARGS.contains(&arg.as_str()))
        .cloned()
        .collect()
}

/// [`DEFAULT_JVM_ARGS`] followed by the user's arguments (without repeating a default).
pub fn with_default_jvm_args(user: &[String]) -> Vec<String> {
    DEFAULT_JVM_ARGS
        .iter()
        .map(|arg| (*arg).to_owned())
        .chain(user_jvm_args(user))
        .collect()
}

/// A request to create a server.
#[derive(Debug, Clone)]
pub struct CreateRequest {
    pub name: String,
    pub mc_version: String,
    pub loader: LoaderKind,
    /// Paper build / Fabric loader version; `None` picks the recommended one.
    pub loader_version: Option<String>,
    pub max_memory_mb: u32,
    /// The user ticked "I accept the Minecraft EULA".
    pub eula_accepted: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error("You must accept the Minecraft EULA to create a server")]
    EulaRequired,
    #[error("The name must be 1 to {MAX_NAME_LEN} characters")]
    InvalidName,
    #[error("Memory must be between {MIN_MEMORY_MB} and {MAX_MEMORY_MB} MB")]
    InvalidMemory,
    #[error("Minecraft {0} does not exist")]
    UnknownVersion(String),
    #[error("{loader} is not available for Minecraft {mc_version}")]
    LoaderUnavailable { loader: LoaderKind, mc_version: String },
    #[error("{loader} version {version} does not exist for Minecraft {mc_version}")]
    UnknownLoaderVersion {
        loader: LoaderKind,
        mc_version: String,
        version: String,
    },
    #[error("the server to copy has not finished installing")]
    SourceNotInstalled,
    #[error("the server isn't installed yet")]
    NotInstalled,
    #[error("the server already runs {0}")]
    VersionUnchanged(String),
    #[error("Minecraft {to} is older than {from}: downgrading can break the world, so it must be confirmed")]
    DowngradeNotConfirmed { from: String, to: String },
    #[error("there is no folder {0:?} in the imports folder")]
    ImportNotFound(String),
    #[error("Instance is not a Minecraft server")]
    NotMinecraft(#[from] SettingsError),
    #[error(transparent)]
    Catalog(CatalogError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Job(#[from] JobError),
}

impl From<CatalogError> for CreateError {
    fn from(error: CatalogError) -> Self {
        match error {
            CatalogError::UnknownVersion(version) => Self::UnknownVersion(version),
            CatalogError::LoaderUnavailable { loader, mc_version } => {
                Self::LoaderUnavailable { loader, mc_version }
            }
            CatalogError::UnknownLoaderVersion {
                loader,
                mc_version,
                version,
            } => Self::UnknownLoaderVersion {
                loader,
                mc_version,
                version,
            },
            other => Self::Catalog(other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MinecraftService {
    catalog: Catalog,
    registry: InstanceRegistry,
    java: JavaRuntimes,
    jobs: JobRunner,
    /// Loader installers run sandboxed, like servers.
    host: HostSandbox,
    paths: MinecraftPaths,
}

/// Where the service keeps shared files.
#[derive(Debug, Clone)]
pub struct MinecraftPaths {
    /// Shared download cache (`<data>/cache/downloads`).
    pub download_cache: PathBuf,
    /// Folders waiting to be imported (`<data>/imports`).
    pub imports: PathBuf,
}

impl MinecraftService {
    pub fn new(
        catalog: Catalog,
        registry: InstanceRegistry,
        java: JavaRuntimes,
        jobs: JobRunner,
        host: HostSandbox,
        paths: MinecraftPaths,
    ) -> Self {
        Self {
            catalog,
            registry,
            java,
            jobs,
            host,
            paths,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Validates, registers the instance and starts its install job.
    pub async fn create(&self, request: CreateRequest) -> Result<(Instance, JobId), CreateError> {
        self.create_then(request, |_, _| async { Ok(()) }).await
    }

    /// Like [`Self::create`], and `after` runs in the same job, once the server files are in and
    /// before the instance counts as installed (so it can't be started halfway through).
    pub async fn create_then<F, Fut>(
        &self,
        request: CreateRequest,
        after: F,
    ) -> Result<(Instance, JobId), CreateError>
    where
        F: FnOnce(Instance, JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), JobFailure>> + Send + 'static,
    {
        let name = validate_basics(&request.name, request.max_memory_mb, request.eula_accepted)?;

        let detail = self.catalog.version_detail(&request.mc_version).await?;
        if request.loader == LoaderKind::Vanilla {
            detail.server_download()?;
        }
        let loader_version = self
            .resolve_loader_version(
                request.loader,
                &request.mc_version,
                request.loader_version.as_deref(),
            )
            .await?;

        let server = MinecraftServer {
            mc_version: request.mc_version.clone(),
            loader: request.loader,
            loader_version,
            eula_accepted_at: Utc::now(),
        };
        let new = NewInstance {
            name: name.to_owned(),
            game: GAME_ID.to_owned(),
            server: server.to_table()?,
            // Provisional: the install job records the real launch (Forge's comes from the files
            // its installer writes). The instance can't start before that anyway.
            launch: LaunchSpec::Jar {
                jar: SERVER_JAR.to_owned(),
                args: vec!["nogui".to_owned()],
            },
            java: JavaSettings {
                major: detail.java_major(),
                runtime: RuntimeChoice::Auto,
                min_memory_mb: request.max_memory_mb.min(1024),
                max_memory_mb: request.max_memory_mb,
                preset: JvmPreset::Aikar,
                extra_jvm_args: DEFAULT_JVM_ARGS.iter().map(|arg| (*arg).to_owned()).collect(),
            },
            behavior: Behavior::default(),
        };

        let instance = self.registry.create(new).await?;
        let job = self.install_then(instance.clone(), after)?;
        Ok((instance, job))
    }

    /// (Re)installs the server files in a background job.
    pub fn install(&self, instance: Instance) -> Result<JobId, CreateError> {
        self.install_then(instance, |_, _| async { Ok(()) })
    }

    fn install_then<F, Fut>(&self, instance: Instance, after: F) -> Result<JobId, CreateError>
    where
        F: FnOnce(Instance, JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), JobFailure>> + Send + 'static,
    {
        let server = MinecraftServer::from_table(&instance.definition.server)?;
        let spec = JobSpec {
            kind: "instance.install",
            title: format!("Installing {}", instance.definition.name),
            instance_id: Some(instance.id()),
        };
        let service = self.clone();
        Ok(self.jobs.spawn(spec, move |job| async move {
            let java_major = instance.definition.java.major;
            job.step(format!("Checking Java {java_major}"));
            let java = service.java.ensure(java_major, &job).await?;
            job.log(format!("Using Java {} at {}", java.version, java.home.display()));

            let target = InstallTarget {
                instance: &instance,
                server: &server,
                java: &java,
                host: &service.host,
                cache_dir: &service.paths.download_cache,
            };
            let launch = install::install(&service.catalog, &target, &job).await?;

            job.step("Setting file ownership");
            instance.layout.assign_ownership(instance.definition.sandbox)?;
            after(instance.clone(), job.clone()).await?;
            service
                .registry
                .update(instance.id(), |definition| {
                    definition.launch = launch;
                    definition.installed_at = Some(Utc::now());
                })
                .await
                .map_err(JobFailure::from)?;
            job.log("Installation complete");
            Ok(())
        })?)
    }

    /// The concrete loader version to record: `wanted` (validated) or the recommended one.
    async fn resolve_loader_version(
        &self,
        loader: LoaderKind,
        mc_version: &str,
        wanted: Option<&str>,
    ) -> Result<Option<String>, CreateError> {
        if !loader.has_versions() {
            return Ok(None);
        }
        let available = self.catalog.loaders_for(mc_version).await?;
        if !available
            .iter()
            .any(|entry| entry.loader == loader && entry.available)
        {
            return Err(CreateError::LoaderUnavailable {
                loader,
                mc_version: mc_version.to_owned(),
            });
        }
        let versions = self.catalog.loader_versions(loader, mc_version).await?;
        let chosen = match wanted {
            Some(wanted) => versions.iter().find(|v| v.id == wanted),
            None => versions.iter().find(|v| v.recommended),
        };
        chosen
            .map(|v| Some(v.id.clone()))
            .ok_or_else(|| CreateError::UnknownLoaderVersion {
                loader,
                mc_version: mc_version.to_owned(),
                version: wanted.unwrap_or("(recommended)").to_owned(),
            })
    }
}

/// The checks every new server passes: consent to the EULA, a name, a sensible memory size.
/// Returns the trimmed name.
fn validate_basics(name: &str, max_memory_mb: u32, eula_accepted: bool) -> Result<&str, CreateError> {
    if !eula_accepted {
        return Err(CreateError::EulaRequired);
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(CreateError::InvalidName);
    }
    if !(MIN_MEMORY_MB..=MAX_MEMORY_MB).contains(&max_memory_mb) {
        return Err(CreateError::InvalidMemory);
    }
    Ok(name)
}

mod change_version;
mod clone;
mod import;

pub use change_version::ChangeVersionRequest;
pub use clone::CloneRequest;
pub use import::{ImportCandidate, ImportRequest};
#[cfg(test)]
mod tests;
