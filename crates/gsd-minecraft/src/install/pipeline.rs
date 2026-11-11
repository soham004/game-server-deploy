//! Installing a server, start to finish: plan → steps → launch → configuration files.

use std::path::Path;

use gsd_core::{
    files::{EntryKind, SafeRelPath, ServerFiles},
    instances::{Instance, definition::LaunchSpec},
    java::JavaRuntime,
    jobs::JobContext,
    supervisor::HostSandbox,
};

use super::{InstallError, execute, plan, server_files};
use crate::{catalog::Catalog, loader::LoaderKind, settings::MinecraftServer};

/// Everything an install needs to know about where and how to install.
#[derive(Debug)]
pub struct InstallTarget<'a> {
    pub instance: &'a Instance,
    pub server: &'a MinecraftServer,
    /// The Java the server (and its installer) runs on.
    pub java: &'a JavaRuntime,
    /// Installers run sandboxed, like the server.
    pub host: &'a HostSandbox,
    /// Shared download cache (`<data>/cache/downloads`).
    pub cache_dir: &'a Path,
}

/// Puts the server files in place and returns how to launch the server.
pub async fn install(
    catalog: &Catalog,
    target: &InstallTarget<'_>,
    job: &JobContext,
) -> Result<LaunchSpec, InstallError> {
    let plan = plan::plan(catalog, target.server).await?;
    let files = ServerFiles::open(target.instance).await?;
    execute::run_steps(catalog, target, &files, &plan.steps, job).await?;
    let launch = execute::resolve_launch(&plan.launch, &files, job).await?;

    job.step("Writing server configuration");
    let definition = &target.instance.definition;
    server_files::write_eula(&files, target.server.eula_accepted_at).await?;
    server_files::seed_properties(&files, definition.ports.game, &definition.name).await?;
    Ok(launch)
}

/// Finishes a server whose files were copied in (import, clone): how to start it, the recorded
/// EULA consent, and its own port. Forge/`NeoForge` start from their `run.sh`; the others from
/// `jar`, which must exist.
pub async fn finish_existing(
    loader: LoaderKind,
    files: &ServerFiles,
    jar: Option<&str>,
    eula_accepted_at: chrono::DateTime<chrono::Utc>,
    port: u16,
    job: &JobContext,
) -> Result<LaunchSpec, InstallError> {
    let launch = if matches!(loader, LoaderKind::Forge | LoaderKind::NeoForge) {
        execute::resolve_launch(&plan::LaunchPlan::FromRunScript, files, job).await?
    } else {
        let jar = jar.ok_or_else(|| InstallError::LaunchNotFound {
            detail: "choose the jar that starts the server".into(),
        })?;
        if !is_file_at_the_top(files, jar).await {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{jar} is not a file in the server folder"),
            });
        }
        execute::resolve_launch(&plan::LaunchPlan::Jar(jar.to_owned()), files, job).await?
    };
    server_files::write_eula(files, eula_accepted_at).await?;
    server_files::point_at_port(files, port).await?;
    Ok(launch)
}

/// A regular file (not a link, not a folder) directly in the server folder.
async fn is_file_at_the_top(files: &ServerFiles, name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return false;
    }
    let Ok(path) = SafeRelPath::parse(name) else {
        return false;
    };
    !path.is_root()
        && files
            .kind(path)
            .await
            .is_ok_and(|kind| kind == Some(EntryKind::File))
}
