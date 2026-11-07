//! Installing a server, start to finish: plan → steps → launch → configuration files.

use std::path::Path;

use gsd_core::{
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
    let server_dir = target.instance.layout.server_dir();
    execute::run_steps(catalog, target, &plan.steps, job).await?;
    let launch = execute::resolve_launch(&plan.launch, &server_dir, job).await?;

    job.step("Writing server configuration");
    let definition = &target.instance.definition;
    server_files::write_eula(&server_dir, target.server.eula_accepted_at).await?;
    server_files::seed_properties(&server_dir, definition.ports.game, &definition.name).await?;
    Ok(launch)
}

/// Finishes a server whose files were copied in (import, clone): how to start it, the recorded
/// EULA consent, and its own port. Forge/`NeoForge` start from their `run.sh`; the others from
/// `jar`, which must exist.
pub async fn finish_existing(
    loader: LoaderKind,
    server_dir: &Path,
    jar: Option<&str>,
    eula_accepted_at: chrono::DateTime<chrono::Utc>,
    port: u16,
    job: &JobContext,
) -> Result<LaunchSpec, InstallError> {
    let launch = if matches!(loader, LoaderKind::Forge | LoaderKind::NeoForge) {
        execute::resolve_launch(&plan::LaunchPlan::FromRunScript, server_dir, job).await?
    } else {
        let jar = jar.ok_or_else(|| InstallError::LaunchNotFound {
            detail: "choose the jar that starts the server".into(),
        })?;
        let is_file = crate::import::is_plain_file(&server_dir.join(jar));
        if !is_file || jar.contains('/') {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{jar} is not a file in the server folder"),
            });
        }
        execute::resolve_launch(&plan::LaunchPlan::Jar(jar.to_owned()), server_dir, job).await?
    };
    server_files::write_eula(server_dir, eula_accepted_at).await?;
    server_files::point_at_port(server_dir, port)?;
    Ok(launch)
}
