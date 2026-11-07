//! What installing a loader involves, as data: the job log shows it, tests assert on it, and
//! one executor (`execute.rs`) runs every loader's plan.

use crate::{
    catalog::{Artifact, Catalog, CatalogError},
    loader::LoaderKind,
    settings::MinecraftServer,
};

/// The steps, then how the finished server is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub steps: Vec<InstallStep>,
    pub launch: LaunchPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStep {
    /// Download `artifact` to `file` in the server directory (through the shared cache when the
    /// artifact has a checksum).
    Download {
        label: String,
        artifact: Artifact,
        file: String,
    },
    /// Run `java -jar <jar> <args>` in the server directory, sandboxed like the server.
    RunInstaller {
        label: String,
        jar: String,
        args: Vec<String>,
    },
    /// Delete files the installer leaves behind (missing ones are fine).
    Remove { files: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
    /// `java -jar <jar> nogui`.
    Jar(String),
    /// Use the launch line of the `run.sh` a Forge/`NeoForge` installer writes (legacy Forge:
    /// its server jar). See `run_script.rs`.
    FromRunScript,
}

/// The jar every server-jar loader is saved as.
pub const SERVER_JAR: &str = "server.jar";
const FABRIC_LAUNCHER: &str = "fabric-server-launch.jar";
const QUILT_LAUNCHER: &str = "quilt-server-launch.jar";

/// Builds the plan for `server` (its loader version is already resolved; see `service.rs`).
pub async fn plan(catalog: &Catalog, server: &MinecraftServer) -> Result<InstallPlan, CatalogError> {
    let loader = server.loader;
    let mc = server.mc_version.as_str();
    let version = server.loader_version.as_deref();
    let artifact = catalog.artifact(loader, mc, version).await?;
    let version_label = version.map(|v| format!(" {v}")).unwrap_or_default();

    let plan = match loader {
        LoaderKind::Vanilla | LoaderKind::Paper | LoaderKind::Purpur => InstallPlan {
            steps: vec![download(
                format!("Downloading {loader}{version_label} for Minecraft {mc}"),
                artifact,
                SERVER_JAR,
            )],
            launch: LaunchPlan::Jar(SERVER_JAR.into()),
        },
        LoaderKind::Fabric => InstallPlan {
            steps: vec![download(
                format!("Downloading Fabric{version_label} for Minecraft {mc}"),
                artifact,
                FABRIC_LAUNCHER,
            )],
            launch: LaunchPlan::Jar(FABRIC_LAUNCHER.into()),
        },
        LoaderKind::Quilt => installer_plan(
            loader,
            artifact,
            vec![
                "install".into(),
                "server".into(),
                mc.into(),
                version.unwrap_or_default().into(),
                "--download-server".into(),
                "--install-dir=.".into(),
            ],
            LaunchPlan::Jar(QUILT_LAUNCHER.into()),
        ),
        // Both install into the working directory. NeoForge also accepts `--installServer`,
        // but documents `--install-server` (checked in its installer, 2026-11-07).
        LoaderKind::Forge => installer_plan(
            loader,
            artifact,
            vec!["--installServer".into()],
            LaunchPlan::FromRunScript,
        ),
        LoaderKind::NeoForge => installer_plan(
            loader,
            artifact,
            vec!["--install-server".into()],
            LaunchPlan::FromRunScript,
        ),
    };
    Ok(plan)
}

/// Download the installer, run it, remove it and its log.
fn installer_plan(
    loader: LoaderKind,
    artifact: Artifact,
    args: Vec<String>,
    launch: LaunchPlan,
) -> InstallPlan {
    let jar = format!("{}-installer.jar", loader.as_str());
    InstallPlan {
        steps: vec![
            download(format!("Downloading the {loader} installer"), artifact, &jar),
            InstallStep::RunInstaller {
                label: format!("Running the {loader} installer (this can take a few minutes)"),
                jar: jar.clone(),
                args,
            },
            InstallStep::Remove {
                files: vec![format!("{jar}.log"), "installer.log".into(), jar],
            },
        ],
        launch,
    }
}

fn download(label: String, artifact: Artifact, file: &str) -> InstallStep {
    InstallStep::Download {
        label,
        artifact,
        file: file.into(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::test_support::{catalog_for, mock_upstream};

    fn server(loader: LoaderKind, version: Option<&str>) -> MinecraftServer {
        MinecraftServer {
            mc_version: "26.3".into(),
            loader,
            loader_version: version.map(Into::into),
            eula_accepted_at: Utc::now(),
        }
    }

    async fn plan_for(loader: LoaderKind, version: Option<&str>) -> InstallPlan {
        let upstream = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        plan(&catalog_for(&upstream, dir.path()), &server(loader, version))
            .await
            .unwrap()
    }

    fn summary(plan: &InstallPlan) -> Vec<String> {
        plan.steps
            .iter()
            .map(|step| match step {
                InstallStep::Download { file, .. } => format!("download {file}"),
                InstallStep::RunInstaller { jar, args, .. } => format!("run {jar} {}", args.join(" ")),
                InstallStep::Remove { files } => format!("remove {}", files.join(" ")),
            })
            .collect()
    }

    #[tokio::test]
    async fn server_jar_loaders_just_download() {
        for (loader, version) in [
            (LoaderKind::Vanilla, None),
            (LoaderKind::Paper, Some("244")),
            (LoaderKind::Purpur, Some("2657")),
        ] {
            let plan = plan_for(loader, version).await;
            assert_eq!(summary(&plan), ["download server.jar"], "{loader}");
            assert_eq!(plan.launch, LaunchPlan::Jar("server.jar".into()));
        }
        let fabric = plan_for(LoaderKind::Fabric, Some("0.20.1")).await;
        assert_eq!(summary(&fabric), ["download fabric-server-launch.jar"]);
        assert_eq!(fabric.launch, LaunchPlan::Jar("fabric-server-launch.jar".into()));
    }

    #[tokio::test]
    async fn quilt_runs_its_installer_for_the_chosen_loader() {
        let plan = plan_for(LoaderKind::Quilt, Some("0.24.0")).await;
        assert_eq!(
            summary(&plan),
            [
                "download quilt-installer.jar",
                "run quilt-installer.jar install server 26.3 0.24.0 --download-server --install-dir=.",
                "remove quilt-installer.jar.log installer.log quilt-installer.jar",
            ]
        );
        assert_eq!(plan.launch, LaunchPlan::Jar("quilt-server-launch.jar".into()));
    }

    #[tokio::test]
    async fn forge_and_neoforge_install_into_the_server_dir_and_launch_from_their_run_script() {
        let forge = plan_for(LoaderKind::Forge, Some("66.1.0")).await;
        assert_eq!(
            summary(&forge)[..2],
            [
                "download forge-installer.jar",
                "run forge-installer.jar --installServer"
            ]
        );
        assert_eq!(forge.launch, LaunchPlan::FromRunScript);

        let neoforge = plan_for(LoaderKind::NeoForge, Some("26.3.0.101")).await;
        assert_eq!(
            summary(&neoforge)[1],
            "run neoforge-installer.jar --install-server"
        );
        assert_eq!(neoforge.launch, LaunchPlan::FromRunScript);
    }

    #[tokio::test]
    async fn labels_name_the_loader_and_versions() {
        let plan = plan_for(LoaderKind::Purpur, Some("2657")).await;
        let InstallStep::Download { label, .. } = &plan.steps[0] else {
            panic!("expected a download");
        };
        assert_eq!(label, "Downloading Purpur 2657 for Minecraft 26.3");
    }
}
