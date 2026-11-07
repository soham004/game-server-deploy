//! Creating Minecraft instances.
//!
//! `create` validates the request against the catalog *synchronously* (so typos fail at once),
//! registers the instance, then installs it in a background job: Java (if missing), the server
//! jar, `eula.txt`, `server.properties`, ownership for the sandbox user. The instance can't be
//! started until the job marks it installed; a failed install can be retried.

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
    jobs::{JobError, JobFailure, JobRunner, JobSpec},
};

use crate::{
    adapter::GAME_ID,
    catalog::{Catalog, CatalogError},
    install::{self, InstallTarget, launch_jar},
    loader::LoaderKind,
    settings::{MinecraftServer, SettingsError},
};

pub const MIN_MEMORY_MB: u32 = 512;
pub const MAX_MEMORY_MB: u32 = 64 * 1024;
const MAX_NAME_LEN: usize = 64;

/// JVM flags every Minecraft server gets: a plain console instead of `JLine`'s interactive prompt
/// (which garbles piped output), while keeping colour codes.
pub const DEFAULT_JVM_ARGS: [&str; 2] = ["-Dterminal.jline=false", "-Dterminal.ansi=true"];

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
    download_cache: PathBuf,
}

impl MinecraftService {
    pub fn new(
        catalog: Catalog,
        registry: InstanceRegistry,
        java: JavaRuntimes,
        jobs: JobRunner,
        download_cache: PathBuf,
    ) -> Self {
        Self {
            catalog,
            registry,
            java,
            jobs,
            download_cache,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Validates, registers the instance and starts its install job.
    pub async fn create(&self, request: CreateRequest) -> Result<(Instance, JobId), CreateError> {
        let name = request.name.trim();
        if !request.eula_accepted {
            return Err(CreateError::EulaRequired);
        }
        if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
            return Err(CreateError::InvalidName);
        }
        if !(MIN_MEMORY_MB..=MAX_MEMORY_MB).contains(&request.max_memory_mb) {
            return Err(CreateError::InvalidMemory);
        }

        let detail = self.catalog.version_detail(&request.mc_version).await?;
        if request.loader == LoaderKind::Vanilla {
            detail.server_download()?;
        }
        let loader_version = self.resolve_loader_version(&request).await?;

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
            launch: LaunchSpec::Jar {
                jar: launch_jar(request.loader).to_owned(),
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
        let job = self.install(instance.clone())?;
        Ok((instance, job))
    }

    /// (Re)installs the server files in a background job.
    pub fn install(&self, instance: Instance) -> Result<JobId, CreateError> {
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
                server: &server,
                server_dir: &instance.layout.server_dir(),
                cache_dir: &service.download_cache,
                port: instance.definition.ports.game,
                motd: &instance.definition.name,
            };
            install::install(&service.catalog, &target, &job).await?;

            job.step("Setting file ownership");
            instance.layout.assign_ownership(instance.definition.sandbox)?;
            service
                .registry
                .update(instance.id(), |definition| {
                    definition.installed_at = Some(Utc::now());
                })
                .await
                .map_err(JobFailure::from)?;
            job.log("Installation complete");
            Ok(())
        })?)
    }

    /// The concrete loader version to record: the requested one (validated) or the recommended one.
    async fn resolve_loader_version(&self, request: &CreateRequest) -> Result<Option<String>, CreateError> {
        if !request.loader.has_versions() {
            return Ok(None);
        }
        let available = self.catalog.loaders_for(&request.mc_version).await?;
        if !available
            .iter()
            .any(|entry| entry.loader == request.loader && entry.available)
        {
            return Err(CreateError::LoaderUnavailable {
                loader: request.loader,
                mc_version: request.mc_version.clone(),
            });
        }
        let versions = self
            .catalog
            .loader_versions(request.loader, &request.mc_version)
            .await?;
        let chosen = match &request.loader_version {
            Some(wanted) => versions.iter().find(|v| &v.id == wanted),
            None => versions.iter().find(|v| v.recommended),
        };
        chosen
            .map(|v| Some(v.id.clone()))
            .ok_or_else(|| CreateError::UnknownLoaderVersion {
                loader: request.loader,
                mc_version: request.mc_version.clone(),
                version: request
                    .loader_version
                    .clone()
                    .unwrap_or_else(|| "(recommended)".into()),
            })
    }
}

#[cfg(test)]
mod tests {
    use gsd_core::{events::EventBus, instances::AllocationPolicy, jobs::JobStatus, net::http_client};

    use super::*;
    use crate::catalog::tests::{FAKE_JAR, catalog_for, mock_upstream};

    struct Fixture {
        service: MinecraftService,
        registry: InstanceRegistry,
        jobs: JobRunner,
        _server: wiremock::MockServer,
        _dir: tempfile::TempDir,
    }

    /// A service wired to the mock upstream, with a fake Java 25 already "installed".
    async fn fixture() -> Fixture {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let java_home = dir.path().join("runtimes/temurin-25-x64");
        std::fs::create_dir_all(java_home.join("bin")).unwrap();
        std::fs::write(java_home.join("bin/java"), "").unwrap();
        std::fs::write(java_home.join("release"), "JAVA_VERSION=\"25.0.1\"").unwrap();

        let events = EventBus::new();
        let policy = AllocationPolicy {
            game_ports: "25565-25570".parse().unwrap(),
            sandbox_uids: 20000..=20010,
            probe_ports: false,
        };
        let (registry, _) = InstanceRegistry::load(
            dir.path().join("instances"),
            dir.path().join("trash"),
            policy,
            events.clone(),
        )
        .unwrap();
        let jobs = JobRunner::new(events);
        let java = JavaRuntimes::with_api(
            dir.path().join("runtimes"),
            http_client().unwrap(),
            server.uri(),
            false,
        );
        let service = MinecraftService::new(
            catalog_for(&server, &dir.path().join("cache/meta")),
            registry.clone(),
            java,
            jobs.clone(),
            dir.path().join("cache/downloads"),
        );
        Fixture {
            service,
            registry,
            jobs,
            _server: server,
            _dir: dir,
        }
    }

    fn request(loader: LoaderKind) -> CreateRequest {
        CreateRequest {
            name: format!("My {loader} server"),
            mc_version: "26.3".into(),
            loader,
            loader_version: None,
            max_memory_mb: 2048,
            eula_accepted: true,
        }
    }

    async fn create_and_wait(fixture: &Fixture, request: CreateRequest) -> Instance {
        let (instance, job) = fixture.service.create(request).await.unwrap();
        let finished = fixture.jobs.wait(job).await.unwrap();
        assert_eq!(
            finished.status,
            JobStatus::Succeeded,
            "{:?} {:?}",
            finished.error,
            fixture.jobs.log(job)
        );
        fixture.registry.get(instance.id()).await.unwrap()
    }

    #[tokio::test]
    async fn installs_a_vanilla_server() {
        let fixture = fixture().await;
        let instance = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
        let server_dir = instance.layout.server_dir();

        assert_eq!(std::fs::read(server_dir.join("server.jar")).unwrap(), FAKE_JAR);
        assert!(
            std::fs::read_to_string(server_dir.join("eula.txt"))
                .unwrap()
                .contains("eula=true")
        );
        let properties = std::fs::read_to_string(server_dir.join("server.properties")).unwrap();
        assert!(properties.contains(&format!("server-port={}", instance.definition.ports.game)));
        assert!(instance.definition.installed_at.is_some());
        assert_eq!(instance.definition.java.major, 25);
        assert_eq!(instance.definition.java.min_memory_mb, 1024);
        assert!(
            instance
                .definition
                .java
                .extra_jvm_args
                .contains(&"-Dterminal.jline=false".to_owned())
        );

        let settings = MinecraftServer::from_table(&instance.definition.server).unwrap();
        assert_eq!(
            (
                settings.mc_version.as_str(),
                settings.loader,
                settings.loader_version
            ),
            ("26.3", LoaderKind::Vanilla, None)
        );
    }

    #[tokio::test]
    async fn installs_paper_with_the_recommended_build() {
        let fixture = fixture().await;
        let instance = create_and_wait(&fixture, request(LoaderKind::Paper)).await;
        let settings = MinecraftServer::from_table(&instance.definition.server).unwrap();
        let recommended = fixture
            .service
            .catalog()
            .loader_versions(LoaderKind::Paper, "26.3")
            .await
            .unwrap()
            .into_iter()
            .find(|v| v.recommended)
            .unwrap();
        assert_eq!(settings.loader_version, Some(recommended.id));
        assert!(instance.layout.server_dir().join("server.jar").is_file());
    }

    #[tokio::test]
    async fn installs_fabric_with_its_launcher_jar() {
        let fixture = fixture().await;
        let instance = create_and_wait(&fixture, request(LoaderKind::Fabric)).await;
        assert!(
            instance
                .layout
                .server_dir()
                .join("fabric-server-launch.jar")
                .is_file()
        );
        assert!(
            matches!(&instance.definition.launch, LaunchSpec::Jar { jar, .. } if jar == "fabric-server-launch.jar")
        );
    }

    #[tokio::test]
    async fn the_download_cache_is_reused() {
        let fixture = fixture().await;
        create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
        let second = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
        let jobs = fixture.jobs.list();
        let log = fixture.jobs.log(jobs[0].id).unwrap();
        assert!(log.iter().any(|line| line.contains("Using cached")), "{log:?}");
        assert!(second.layout.server_dir().join("server.jar").is_file());
    }

    #[tokio::test]
    async fn rejects_invalid_requests_before_creating_anything() {
        type Expectation = fn(&CreateError) -> bool;
        let fixture = fixture().await;
        let cases: Vec<(CreateRequest, Expectation)> = vec![
            (
                CreateRequest {
                    eula_accepted: false,
                    ..request(LoaderKind::Vanilla)
                },
                |e| matches!(e, CreateError::EulaRequired),
            ),
            (
                CreateRequest {
                    name: "   ".into(),
                    ..request(LoaderKind::Vanilla)
                },
                |e| matches!(e, CreateError::InvalidName),
            ),
            (
                CreateRequest {
                    max_memory_mb: 64,
                    ..request(LoaderKind::Vanilla)
                },
                |e| matches!(e, CreateError::InvalidMemory),
            ),
            (
                CreateRequest {
                    mc_version: "9.9.9".into(),
                    ..request(LoaderKind::Vanilla)
                },
                |e| matches!(e, CreateError::UnknownVersion(_)),
            ),
            (
                CreateRequest {
                    loader_version: Some("0.0.0-nope".into()),
                    ..request(LoaderKind::Fabric)
                },
                |e| matches!(e, CreateError::UnknownLoaderVersion { .. }),
            ),
            (
                CreateRequest {
                    mc_version: "1.0".into(),
                    ..request(LoaderKind::Vanilla)
                },
                |e| matches!(e, CreateError::Catalog(CatalogError::NoServerDownload(_))),
            ),
            (
                CreateRequest {
                    mc_version: "1.0".into(),
                    ..request(LoaderKind::Paper)
                },
                |e| matches!(e, CreateError::LoaderUnavailable { .. }),
            ),
        ];
        for (request, expected) in cases {
            let error = fixture.service.create(request.clone()).await.unwrap_err();
            assert!(expected(&error), "{request:?} gave {error:?}");
        }
        assert!(fixture.registry.list().await.is_empty(), "nothing was registered");
    }

    #[tokio::test]
    async fn failed_installs_leave_an_uninstalled_instance() {
        let fixture = fixture().await;
        // No Java 25 and an Adoptium mock with nothing for it: the install job must fail.
        std::fs::remove_dir_all(fixture.service.java.managed_dir().join("temurin-25-x64")).unwrap();
        fixture.service.java.list(true).await;

        let (instance, job) = fixture
            .service
            .create(request(LoaderKind::Vanilla))
            .await
            .unwrap();
        let finished = fixture.jobs.wait(job).await.unwrap();
        assert_eq!(finished.status, JobStatus::Failed);
        assert!(
            fixture
                .registry
                .get(instance.id())
                .await
                .unwrap()
                .definition
                .installed_at
                .is_none()
        );
    }
}
