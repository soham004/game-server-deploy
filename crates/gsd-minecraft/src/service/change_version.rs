//! Changing a server's Minecraft version or loader version (the loader itself stays). In a job:
//! a full safety backup first, then the new version installed over the old files, the old
//! loader version's own folder removed, and `instance.toml` updated only once that worked. If
//! something fails after the backup, the error names the backup to restore.
//!
//! Going to an older Minecraft version can corrupt a world (newer chunks don't load in older
//! versions), so it must be confirmed (`allow_downgrade`). The caller makes sure the server is
//! stopped.

use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use gsd_core::{
    backups::{BackupKind, BackupRequest, BackupScope, BackupService},
    ids::{InstanceId, JobId, UserId},
    instances::{
        Instance,
        definition::{LaunchSpec, RuntimeChoice},
        registry::RegistryError,
    },
    jobs::{JobContext, JobFailure, JobSpec, error_chain},
};

use super::{CreateError, MinecraftService};
use crate::{
    install::{self, InstallTarget},
    loader::LoaderKind,
    settings::MinecraftServer,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeVersionRequest {
    pub mc_version: String,
    /// The loader version; `None` picks the recommended one for `mc_version`.
    pub loader_version: Option<String>,
    /// Going to an older Minecraft version was confirmed.
    pub allow_downgrade: bool,
    pub by: Option<UserId>,
}

impl MinecraftService {
    /// Checks the change against the catalog, then makes it in a job.
    pub async fn change_version(
        &self,
        id: InstanceId,
        request: ChangeVersionRequest,
        backups: &BackupService,
    ) -> Result<JobId, CreateError> {
        let instance = self
            .registry
            .get(id)
            .await
            .ok_or(CreateError::Registry(RegistryError::NotFound))?;
        if instance.definition.installed_at.is_none() {
            return Err(CreateError::NotInstalled);
        }
        let current = MinecraftServer::from_table(&instance.definition.server)?;
        let target = self.target(&current, &request).await?;
        let java_major = self
            .catalog
            .version_detail(&target.mc_version)
            .await?
            .java_major();

        let spec = JobSpec {
            kind: "instance.change_version",
            title: format!(
                "Changing {} to {}",
                instance.definition.name,
                target.software_label()
            ),
            instance_id: Some(id),
        };
        let (service, backups) = (self.clone(), backups.clone());
        Ok(self.jobs.spawn(spec, move |job| async move {
            let backup = service
                .safety_backup(&instance, &current, &target, request.by, &backups, &job)
                .await?;
            service
                .install_version(&instance, &target, java_major, &job)
                .await
                .map_err(|error| {
                    JobFailure::msg(format!(
                        "{error}. The server's files may be partly changed: restore the backup {backup} to go back."
                    ))
                })
        })?)
    }

    /// The validated new `[server]` table: same loader, the new versions.
    async fn target(
        &self,
        current: &MinecraftServer,
        request: &ChangeVersionRequest,
    ) -> Result<MinecraftServer, CreateError> {
        let detail = self.catalog.version_detail(&request.mc_version).await?;
        if current.loader == LoaderKind::Vanilla {
            detail.server_download()?;
        }
        let loader_version = self
            .resolve_loader_version(
                current.loader,
                &request.mc_version,
                request.loader_version.as_deref(),
            )
            .await?;
        let target = MinecraftServer {
            mc_version: request.mc_version.clone(),
            loader: current.loader,
            loader_version,
            eula_accepted_at: current.eula_accepted_at,
        };
        if target == *current {
            return Err(CreateError::VersionUnchanged(current.software_label()));
        }
        if !request.allow_downgrade && self.is_older(&target.mc_version, &current.mc_version).await? {
            return Err(CreateError::DowngradeNotConfirmed {
                from: current.mc_version.clone(),
                to: target.mc_version.clone(),
            });
        }
        Ok(target)
    }

    /// Whether Minecraft `candidate` was released before `reference`.
    async fn is_older(&self, candidate: &str, reference: &str) -> Result<bool, CreateError> {
        let versions = self.catalog.game_versions().await?;
        let released = |id: &str| versions.iter().find(|v| v.id == id).map(|v| v.released);
        Ok(matches!((released(candidate), released(reference)), (Some(a), Some(b)) if a < b))
    }

    /// A full backup, so the change can be undone. Returns its file name.
    async fn safety_backup(
        &self,
        instance: &Instance,
        current: &MinecraftServer,
        target: &MinecraftServer,
        by: Option<UserId>,
        backups: &BackupService,
        job: &JobContext,
    ) -> Result<String, JobFailure> {
        job.step("Backing up everything first");
        let request = BackupRequest {
            scope: BackupScope::Full,
            kind: BackupKind::PreVersionChange,
            note: Some(format!(
                "Before changing from {} to {}",
                current.software_label(),
                target.software_label()
            )),
            by,
        };
        let record = backups.create(instance, &request, job).await.map_err(|error| {
            JobFailure::msg(format!(
                "the safety backup failed, so nothing was changed: {}",
                error_chain(&error)
            ))
        })?;
        job.log(format!("Saved {}", record.file_name));
        Ok(record.file_name)
    }

    /// Installs `target` over the current files and records it.
    async fn install_version(
        &self,
        instance: &Instance,
        target: &MinecraftServer,
        java_major: u32,
        job: &JobContext,
    ) -> Result<(), JobFailure> {
        job.step(format!("Checking Java {java_major}"));
        let java = self.java.ensure(java_major, job).await?;
        let install_target = InstallTarget {
            instance,
            server: target,
            java: &java,
            host: &self.host,
            cache_dir: &self.paths.download_cache,
        };
        let launch = install::install(&self.catalog, &install_target, job).await?;
        remove_old_loader_files(instance, &launch, job).await;

        job.step("Setting file ownership");
        instance.layout.assign_ownership(instance.definition.sandbox)?;
        let server = target.to_table()?;
        let keeps_runtime = self.runtime_still_fits(instance, java_major).await;
        self.registry
            .update(instance.id(), |definition| {
                definition.server = server;
                definition.java.major = java_major;
                if !keeps_runtime {
                    definition.java.runtime = RuntimeChoice::Auto;
                }
                definition.launch = launch;
                definition.installed_at = Some(Utc::now());
            })
            .await
            .map_err(JobFailure::from)?;
        if !keeps_runtime {
            job.log(format!(
                "The chosen Java runtime is older than Java {java_major}; the server now picks one automatically."
            ));
        }
        let build = target
            .loader_version
            .as_deref()
            .map(|build| format!(" ({build})"))
            .unwrap_or_default();
        job.log(format!("Now on {}{build}", target.software_label()));
        Ok(())
    }

    /// A specifically chosen Java runtime must still be new enough for the new version.
    async fn runtime_still_fits(&self, instance: &Instance, java_major: u32) -> bool {
        let RuntimeChoice::Runtime { id } = &instance.definition.java.runtime else {
            return true;
        };
        self.java
            .list(false)
            .await
            .iter()
            .any(|runtime| &runtime.id == id && runtime.major >= java_major)
    }
}

/// Removes the old loader version's own folder (see [`old_loader_folder`]).
async fn remove_old_loader_files(instance: &Instance, new_launch: &LaunchSpec, job: &JobContext) {
    let Some(old_folder) = old_loader_folder(&instance.definition.launch, new_launch) else {
        return;
    };
    let path = instance.layout.server_dir().join(&old_folder);
    match tokio::fs::remove_dir_all(&path).await {
        Ok(()) => job.log(format!(
            "Removed the old version's files: {}",
            old_folder.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => job.log(format!("Could not remove {}: {error}", old_folder.display())),
    }
}

/// Forge and `NeoForge` keep each version in its own folder under `libraries/` (the one holding
/// the args file the server starts from). After a change, the old version's folder can go;
/// everything else (worlds, configs, mods, shared libraries) stays. Only a plain relative path
/// inside `libraries/` qualifies.
fn old_loader_folder(old: &LaunchSpec, new: &LaunchSpec) -> Option<PathBuf> {
    let (LaunchSpec::ArgsFiles { args_file: old, .. }, LaunchSpec::ArgsFiles { args_file: new, .. }) =
        (old, new)
    else {
        return None;
    };
    let old_folder = Path::new(old).parent()?;
    let is_plain = old_folder
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    let qualifies = is_plain && old_folder.starts_with("libraries") && old_folder != Path::new("libraries");
    (qualifies && Some(old_folder) != Path::new(new).parent()).then(|| old_folder.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_file(path: &str) -> LaunchSpec {
        LaunchSpec::ArgsFiles {
            jvm_args_file: "user_jvm_args.txt".into(),
            args_file: path.into(),
            args: vec![],
        }
    }

    #[test]
    fn only_the_old_versions_own_folder_under_libraries_goes() {
        let old = args_file("libraries/net/minecraftforge/forge/26.3-66.0.0/unix_args.txt");
        let new = args_file("libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt");
        assert_eq!(
            old_loader_folder(&old, &new),
            Some(PathBuf::from("libraries/net/minecraftforge/forge/26.3-66.0.0"))
        );
        assert_eq!(old_loader_folder(&new, &new), None, "same version");
        let jar = LaunchSpec::Jar {
            jar: "server.jar".into(),
            args: vec![],
        };
        assert_eq!(old_loader_folder(&jar, &new), None);
        for odd in [
            "../outside/unix_args.txt",
            "/etc/unix_args.txt",
            "libraries/unix_args.txt",
            "config/unix_args.txt",
            "libraries/../x/unix_args.txt",
        ] {
            assert_eq!(old_loader_folder(&args_file(odd), &new), None, "{odd}");
        }
    }
}
