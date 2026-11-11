//! Creating and installing servers for every loader, against the mock upstream (recorded API
//! responses) and a fake `java` that behaves like the loader installers.

use std::{collections::HashMap, sync::Arc};

use gsd_core::{
    events::EventBus,
    game::GameAdapter,
    instances::AllocationPolicy,
    jobs::{JobSnapshot, JobStatus},
    net::http_client,
    supervisor::{SandboxSupport, Supervisors},
};

use super::*;
use crate::test_support::{catalog_for, fake_jar, install_fake_java, mock_upstream};

struct Fixture {
    service: MinecraftService,
    registry: InstanceRegistry,
    jobs: JobRunner,
    java_home: PathBuf,
    _server: wiremock::MockServer,
    _dir: tempfile::TempDir,
}

/// A service wired to the mock upstream, with a fake Java 25 already "installed".
async fn fixture() -> Fixture {
    fixture_with(HostSandbox::disabled()).await
}

async fn fixture_with(host: HostSandbox) -> Fixture {
    let server = mock_upstream().await;
    let dir = tempfile::tempdir().unwrap();
    let java_home = dir.path().join("runtimes/temurin-25-x64");
    install_fake_java(&java_home);

    let events = EventBus::new();
    let policy = AllocationPolicy {
        game_ports: "25565-25570".parse().unwrap(),
        control_ports: "35565-35570".parse().unwrap(),
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
    let jobs = JobRunner::new(events.clone());
    let java = JavaRuntimes::with_api(
        dir.path().join("runtimes"),
        http_client().unwrap(),
        server.uri(),
        false,
    );
    let adapter: Arc<dyn GameAdapter> = Arc::new(crate::MinecraftAdapter);
    let instances = InstanceService::new(
        registry.clone(),
        Supervisors::new(events.clone(), host),
        java.clone(),
        jobs.clone(),
        HashMap::from([(GAME_ID.to_owned(), adapter)]),
    );
    let service = MinecraftService::new(
        catalog_for(&server, &dir.path().join("cache/meta")),
        instances,
        java,
        jobs.clone(),
        MinecraftPaths {
            download_cache: dir.path().join("cache/downloads"),
            imports: dir.path().join("imports"),
        },
    );
    Fixture {
        service,
        registry,
        jobs,
        java_home,
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

async fn create_and_finish(
    fixture: &Fixture,
    request: CreateRequest,
) -> (Instance, JobSnapshot, Vec<String>) {
    let (instance, job) = fixture.service.create(request).await.unwrap();
    let finished = fixture.jobs.wait(job).await.unwrap();
    let log = fixture.jobs.log(job).unwrap();
    (fixture.registry.get(instance.id()).await.unwrap(), finished, log)
}

async fn create_and_wait(fixture: &Fixture, request: CreateRequest) -> Instance {
    let (instance, finished, log) = create_and_finish(fixture, request).await;
    assert_eq!(
        finished.status,
        JobStatus::Succeeded,
        "{:?} {log:#?}",
        finished.error
    );
    instance
}

fn launch_jar(instance: &Instance) -> &str {
    match &instance.definition.launch {
        LaunchSpec::Jar { jar, .. } => jar,
        other @ LaunchSpec::ArgsFiles { .. } => panic!("expected a jar launch, got {other:?}"),
    }
}

#[tokio::test]
async fn installs_a_vanilla_server() {
    let fixture = fixture().await;
    let instance = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
    let server_dir = instance.layout.server_dir();

    assert_eq!(std::fs::read(server_dir.join("server.jar")).unwrap(), fake_jar());
    assert!(
        std::fs::read_to_string(server_dir.join("eula.txt"))
            .unwrap()
            .contains("eula=true")
    );
    let properties = std::fs::read_to_string(server_dir.join("server.properties")).unwrap();
    assert!(properties.contains(&format!("server-port={}", instance.definition.ports.game)));
    assert!(instance.definition.installed_at.is_some());
    assert_eq!(launch_jar(&instance), "server.jar");
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
async fn server_jar_loaders_record_the_recommended_version() {
    let fixture = fixture().await;
    for loader in [LoaderKind::Paper, LoaderKind::Purpur, LoaderKind::Fabric] {
        let instance = create_and_wait(&fixture, request(loader)).await;
        let settings = MinecraftServer::from_table(&instance.definition.server).unwrap();
        let recommended = fixture
            .service
            .catalog()
            .loader_versions(loader, "26.3")
            .await
            .unwrap()
            .into_iter()
            .find(|v| v.recommended)
            .unwrap();
        assert_eq!(settings.loader_version, Some(recommended.id), "{loader}");
        let jar = launch_jar(&instance).to_owned();
        assert_eq!(
            std::fs::read(instance.layout.server_dir().join(&jar)).unwrap(),
            fake_jar(),
            "{loader}"
        );
    }
}

#[tokio::test]
async fn fabric_launches_its_launcher_jar() {
    let fixture = fixture().await;
    let instance = create_and_wait(&fixture, request(LoaderKind::Fabric)).await;
    assert_eq!(launch_jar(&instance), "fabric-server-launch.jar");
}

#[cfg(unix)]
#[tokio::test]
async fn forge_runs_its_installer_and_launches_from_the_args_files() {
    let fixture = fixture().await;
    let (instance, finished, log) = create_and_finish(&fixture, request(LoaderKind::Forge)).await;
    assert_eq!(
        finished.status,
        JobStatus::Succeeded,
        "{:?} {log:#?}",
        finished.error
    );

    assert_eq!(
        instance.definition.launch,
        LaunchSpec::ArgsFiles {
            jvm_args_file: "user_jvm_args.txt".into(),
            args_file: "libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt".into(),
            args: vec!["nogui".into()],
        }
    );
    let settings = MinecraftServer::from_table(&instance.definition.server).unwrap();
    assert_eq!(
        settings.loader_version.as_deref(),
        Some("66.1.0"),
        "the ★ recommended build"
    );

    let server_dir = instance.layout.server_dir();
    for leftover in [
        "forge-installer.jar",
        "forge-installer.jar.log",
        "run.sh",
        "run.bat",
    ] {
        assert!(
            !server_dir.join(leftover).exists(),
            "{leftover} should be removed"
        );
    }
    assert!(server_dir.join("eula.txt").is_file());
    assert!(
        log.iter()
            .any(|line| line.contains("fake installer:") && line.ends_with("--installServer")),
        "{log:#?}"
    );
    assert!(
        log.iter()
            .any(|line| line.ends_with("The server installed successfully"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn neoforge_and_quilt_install_through_their_installers() {
    let fixture = fixture().await;
    let neoforge = create_and_wait(&fixture, request(LoaderKind::NeoForge)).await;
    assert!(matches!(
        &neoforge.definition.launch,
        LaunchSpec::ArgsFiles { args_file, .. } if args_file == "libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt"
    ));

    let (quilt, finished, log) = create_and_finish(&fixture, request(LoaderKind::Quilt)).await;
    assert_eq!(finished.status, JobStatus::Succeeded, "{log:#?}");
    assert_eq!(launch_jar(&quilt), "quilt-server-launch.jar");
    assert!(!quilt.layout.server_dir().join("quilt-installer.jar").exists());
    assert!(
        log.iter()
            .any(|line| line.ends_with("install server 26.3 0.30.2 --download-server --install-dir=.")),
        "{log:#?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_failing_installer_fails_the_job_with_its_output_in_the_log() {
    let fixture = fixture().await;
    std::fs::write(fixture.java_home.join("fail-installer"), "").unwrap();
    let (instance, finished, log) = create_and_finish(&fixture, request(LoaderKind::Forge)).await;

    assert_eq!(finished.status, JobStatus::Failed);
    let error = finished.error.unwrap();
    assert!(
        error.contains("exit code 3") && error.contains("job log"),
        "{error}"
    );
    assert!(
        log.iter()
            .any(|line| line.contains("IllegalStateException: boom")),
        "{log:#?}"
    );
    assert!(instance.definition.installed_at.is_none(), "can't be started");

    // Retrying after the cause is fixed works.
    std::fs::remove_file(fixture.java_home.join("fail-installer")).unwrap();
    let job = fixture.service.install(instance.clone()).unwrap();
    assert_eq!(fixture.jobs.wait(job).await.unwrap().status, JobStatus::Succeeded);
    let retried = fixture.registry.get(instance.id()).await.unwrap();
    assert!(retried.definition.installed_at.is_some());
    assert!(matches!(retried.definition.launch, LaunchSpec::ArgsFiles { .. }));
}

#[tokio::test]
async fn installers_do_not_run_when_the_sandbox_is_unusable() {
    let problem = "Servers can't start: move the data folder";
    let fixture = fixture_with(HostSandbox::new(SandboxSupport::none(), Some(problem.into()))).await;

    let (forge, finished, _) = create_and_finish(&fixture, request(LoaderKind::Forge)).await;
    assert_eq!(finished.status, JobStatus::Failed);
    assert_eq!(finished.error.as_deref(), Some(problem));
    assert!(forge.definition.installed_at.is_none());

    // Server-jar loaders run no code while installing.
    create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
}

#[tokio::test]
async fn the_download_cache_is_reused() {
    let fixture = fixture().await;
    create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
    let (second, _, log) = create_and_finish(&fixture, request(LoaderKind::Vanilla)).await;
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
                loader_version: Some("1.2.3".into()),
                ..request(LoaderKind::Forge)
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
                ..request(LoaderKind::NeoForge)
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
    std::fs::remove_dir_all(&fixture.java_home).unwrap();
    fixture.service.java.list(true).await;

    let (instance, finished, _) = create_and_finish(&fixture, request(LoaderKind::Vanilla)).await;
    assert_eq!(finished.status, JobStatus::Failed);
    assert!(instance.definition.installed_at.is_none());
}

fn write(dir: &std::path::Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

#[tokio::test]
async fn clones_copy_files_and_get_their_own_port_without_worlds_by_default() {
    let fixture = fixture().await;
    let source = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
    let port = source.definition.ports.game;
    write(
        &source.layout.server_dir(),
        &[
            (
                "server.properties",
                &format!("level-name=survival\nserver-port={port}\nquery.port={port}\nmotd=Hi\n"),
            ),
            ("survival/level.dat", "world"),
            ("survival_nether/DIM-1/x", "nether"),
            ("survival_the_end/DIM1/x", "end"),
            ("logs/latest.log", "log"),
            ("crash-reports/crash.txt", "crash"),
            ("config/mod.toml", "setting = 1"),
        ],
    );

    let (clone, job) = fixture
        .service
        .clone_instance(
            source.id(),
            CloneRequest {
                name: "Copy".into(),
                include_worlds: false,
            },
        )
        .await
        .unwrap();
    let finished = fixture.jobs.wait(job).await.unwrap();
    assert_eq!(finished.status, JobStatus::Succeeded, "{:?}", finished.error);
    let clone = fixture.registry.get(clone.id()).await.unwrap();

    assert_ne!(clone.id(), source.id());
    assert_ne!(clone.definition.ports.game, port, "its own port");
    assert_ne!(
        clone.definition.sandbox, source.definition.sandbox,
        "its own sandbox user"
    );
    assert!(clone.definition.installed_at.is_some());
    assert!(!clone.definition.behavior.auto_start);
    assert_eq!(clone.definition.java, source.definition.java);
    assert_eq!(clone.definition.launch, source.definition.launch);

    let dir = clone.layout.server_dir();
    assert_eq!(
        std::fs::read_to_string(dir.join("config/mod.toml")).unwrap(),
        "setting = 1"
    );
    assert!(dir.join("server.jar").is_file() && dir.join("eula.txt").is_file());
    for left_out in [
        "survival",
        "survival_nether",
        "survival_the_end",
        "logs",
        "crash-reports",
    ] {
        assert!(!dir.join(left_out).exists(), "{left_out} was copied");
    }
    let properties = std::fs::read_to_string(dir.join("server.properties")).unwrap();
    let new_port = clone.definition.ports.game;
    assert_eq!(
        properties,
        format!("level-name=survival\nserver-port={new_port}\nquery.port={new_port}\nmotd=Hi\n")
    );
    // The source is untouched.
    assert!(source.layout.server_dir().join("survival/level.dat").is_file());
}

#[tokio::test]
async fn clones_can_include_the_worlds() {
    let fixture = fixture().await;
    let source = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
    write(
        &source.layout.server_dir(),
        &[("world/level.dat", "world"), ("logs/latest.log", "log")],
    );
    let (clone, job) = fixture
        .service
        .clone_instance(
            source.id(),
            CloneRequest {
                name: "With world".into(),
                include_worlds: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(fixture.jobs.wait(job).await.unwrap().status, JobStatus::Succeeded);
    assert!(clone.layout.server_dir().join("world/level.dat").is_file());
    assert!(!clone.layout.server_dir().join("logs").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn clones_need_an_installed_source_and_a_valid_name() {
    let fixture = fixture().await;
    std::fs::write(fixture.java_home.join("fail-installer"), "").unwrap();
    let (broken, job) = fixture.service.create(request(LoaderKind::Forge)).await.unwrap();
    fixture.jobs.wait(job).await.unwrap();
    let result = fixture
        .service
        .clone_instance(
            broken.id(),
            CloneRequest {
                name: "Copy".into(),
                include_worlds: false,
            },
        )
        .await;
    assert!(
        matches!(result, Err(CreateError::SourceNotInstalled)),
        "{result:?}"
    );

    std::fs::remove_file(fixture.java_home.join("fail-installer")).unwrap();
    let source = create_and_wait(&fixture, request(LoaderKind::Vanilla)).await;
    let result = fixture
        .service
        .clone_instance(
            source.id(),
            CloneRequest {
                name: "  ".into(),
                include_worlds: false,
            },
        )
        .await;
    assert!(matches!(result, Err(CreateError::InvalidName)));
    let missing = fixture
        .service
        .clone_instance(
            gsd_core::ids::InstanceId::generate(),
            CloneRequest {
                name: "x".into(),
                include_worlds: false,
            },
        )
        .await;
    assert!(missing.is_err());
    assert_eq!(
        fixture.registry.list().await.len(),
        2,
        "nothing extra was registered"
    );
}

fn import_request(folder: &str, loader: LoaderKind, jar: Option<&str>) -> ImportRequest {
    ImportRequest {
        folder: folder.into(),
        name: format!("Imported {folder}"),
        mc_version: "26.3".into(),
        loader,
        loader_version: None,
        jar: jar.map(Into::into),
        max_memory_mb: 2048,
        eula_accepted: true,
    }
}

fn imports_dir(fixture: &Fixture) -> PathBuf {
    fixture
        .java_home
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("imports")
}

#[tokio::test]
async fn lists_importable_folders_with_what_they_look_like() {
    let fixture = fixture().await;
    let imports = imports_dir(&fixture);
    write(
        &imports,
        &[
            ("fabric-smp/fabric-server-launch.jar", ""),
            ("plain/server.jar", ""),
            ("loose-file.txt", ""),
        ],
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc", imports.join("link-to-etc")).unwrap();

    let candidates = fixture.service.import_candidates().await;
    let summary: Vec<(String, Option<LoaderKind>)> = candidates
        .iter()
        .map(|c| (c.folder.clone(), c.detected.loader))
        .collect();
    assert_eq!(
        summary,
        [
            ("fabric-smp".to_owned(), Some(LoaderKind::Fabric)),
            ("plain".to_owned(), Some(LoaderKind::Vanilla))
        ],
        "only real folders; links and files are ignored"
    );
}

#[tokio::test]
async fn imports_a_folder_as_a_new_sandboxed_instance() {
    let fixture = fixture().await;
    let imports = imports_dir(&fixture);
    write(
        &imports,
        &[
            ("old-smp/server.jar", "jar"),
            ("old-smp/world/level.dat", "world"),
            ("old-smp/server.properties", "motd=Old\nserver-port=25565\n"),
            ("old-smp/eula.txt", "eula=false\n"),
        ],
    );
    let (instance, job) = fixture
        .service
        .import(import_request("old-smp", LoaderKind::Vanilla, Some("server.jar")))
        .await
        .unwrap();
    let finished = fixture.jobs.wait(job).await.unwrap();
    assert_eq!(
        finished.status,
        JobStatus::Succeeded,
        "{:?} {:?}",
        finished.error,
        fixture.jobs.log(job)
    );
    let instance = fixture.registry.get(instance.id()).await.unwrap();
    let dir = instance.layout.server_dir();

    assert!(instance.definition.installed_at.is_some());
    assert_eq!(launch_jar(&instance), "server.jar");
    assert_eq!(
        std::fs::read_to_string(dir.join("world/level.dat")).unwrap(),
        "world"
    );
    assert!(
        std::fs::read_to_string(dir.join("eula.txt"))
            .unwrap()
            .contains("eula=true"),
        "consent recorded"
    );
    let port = instance.definition.ports.game;
    assert_eq!(
        std::fs::read_to_string(dir.join("server.properties")).unwrap(),
        format!("motd=Old\nserver-port={port}\nquery.port={port}\n")
    );
    assert!(
        imports.join("old-smp/world/level.dat").is_file(),
        "the original stays"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn imports_neoforge_from_its_run_script() {
    let fixture = fixture().await;
    let imports = imports_dir(&fixture);
    let args = "libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt";
    write(
        &imports,
        &[
            (
                "neo/run.sh",
                &crate::test_support::fixture_text("neoforge_26.3_run.sh"),
            ),
            ("neo/user_jvm_args.txt", ""),
            (
                &format!("neo/{args}"),
                "-Djava.net.preferIPv6Addresses=system\n-DlibraryDirectory=libraries\n",
            ),
        ],
    );
    let (instance, job) = fixture
        .service
        .import(import_request("neo", LoaderKind::NeoForge, None))
        .await
        .unwrap();
    assert_eq!(fixture.jobs.wait(job).await.unwrap().status, JobStatus::Succeeded);
    let instance = fixture.registry.get(instance.id()).await.unwrap();
    assert!(
        matches!(&instance.definition.launch, LaunchSpec::ArgsFiles { args_file, .. } if args_file == args)
    );
    let unix_args = std::fs::read_to_string(instance.layout.server_dir().join(args)).unwrap();
    assert_eq!(
        unix_args, "-DlibraryDirectory=libraries\n",
        "the sandbox-breaking flag is removed"
    );
}

#[tokio::test]
async fn imports_refuse_unknown_folders_and_need_a_jar() {
    let fixture = fixture().await;
    let imports = imports_dir(&fixture);
    write(&imports, &[("two-jars/a.jar", ""), ("two-jars/b.jar", "")]);
    for folder in ["missing", "../instances", "two-jars/a.jar", ""] {
        let result = fixture
            .service
            .import(import_request(folder, LoaderKind::Vanilla, Some("a.jar")))
            .await;
        assert!(
            matches!(result, Err(CreateError::ImportNotFound(_))),
            "{folder:?}: {result:?}"
        );
    }
    let no_consent = ImportRequest {
        eula_accepted: false,
        ..import_request("two-jars", LoaderKind::Vanilla, Some("a.jar"))
    };
    assert!(matches!(
        fixture.service.import(no_consent).await,
        Err(CreateError::EulaRequired)
    ));

    // Without a jar the job fails with an explanation, and the instance can't start.
    let (instance, job) = fixture
        .service
        .import(import_request("two-jars", LoaderKind::Vanilla, None))
        .await
        .unwrap();
    let finished = fixture.jobs.wait(job).await.unwrap();
    assert_eq!(finished.status, JobStatus::Failed);
    assert!(finished.error.unwrap().contains("choose the jar"));
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
