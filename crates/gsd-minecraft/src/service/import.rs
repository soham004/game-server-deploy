//! Importing a server folder from `<data>/imports/`: list what's there (with what each folder
//! looks like), then copy one into a new instance in a background job. The original stays where
//! it is; nothing is downloaded (the folder already holds the server).

use chrono::Utc;
use gsd_core::{
    files::{SafeRelPath, copy_server_dir},
    ids::JobId,
    instances::{
        Instance, NewInstance,
        definition::{Behavior, JavaSettings, JvmPreset, LaunchSpec, RuntimeChoice},
    },
    jobs::{JobFailure, JobSpec},
};
use serde::Serialize;

use super::{CreateError, DEFAULT_JVM_ARGS, MinecraftService, validate_basics};
use crate::{
    import::{Detected, detect},
    install::{SERVER_JAR, finish_existing},
    loader::LoaderKind,
    settings::MinecraftServer,
};

/// A folder waiting to be imported.
#[derive(Debug, Clone, Serialize)]
pub struct ImportCandidate {
    pub folder: String,
    pub detected: Detected,
}

/// What the user confirmed about a folder.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// A folder directly inside `<data>/imports/`.
    pub folder: String,
    pub name: String,
    pub mc_version: String,
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    /// For jar-launched loaders: the jar that starts it.
    pub jar: Option<String>,
    pub max_memory_mb: u32,
    pub eula_accepted: bool,
}

impl MinecraftService {
    /// The folders in `<data>/imports/` and what each looks like (links are ignored).
    pub async fn import_candidates(&self) -> Vec<ImportCandidate> {
        let root = self.paths.imports.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(&root) else {
                return Vec::new();
            };
            let mut candidates: Vec<ImportCandidate> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|entry| {
                    let folder = entry.file_name().into_string().ok()?;
                    Some(ImportCandidate {
                        detected: detect(&entry.path()),
                        folder,
                    })
                })
                .collect();
            candidates.sort_by(|a, b| a.folder.cmp(&b.folder));
            candidates
        })
        .await
        .unwrap_or_default()
    }

    /// Registers the instance and starts the copy job.
    pub async fn import(&self, request: ImportRequest) -> Result<(Instance, JobId), CreateError> {
        let name = validate_basics(&request.name, request.max_memory_mb, request.eula_accepted)?.to_owned();
        let source = self.import_source(&request.folder)?;
        let detail = self.catalog.version_detail(&request.mc_version).await?;
        let server = MinecraftServer {
            mc_version: request.mc_version.clone(),
            loader: request.loader,
            loader_version: request.loader_version.clone().filter(|v| !v.trim().is_empty()),
            eula_accepted_at: Utc::now(),
        };
        let instance = self
            .registry
            .create(NewInstance {
                name,
                game: crate::adapter::GAME_ID.to_owned(),
                server: server.to_table()?,
                launch: LaunchSpec::Jar {
                    jar: request.jar.clone().unwrap_or_else(|| SERVER_JAR.to_owned()),
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
            })
            .await?;
        let job = self.import_job(source, instance.clone(), server, request.jar)?;
        Ok((instance, job))
    }

    /// `<data>/imports/<folder>`, which must be a real folder (one name, no links).
    fn import_source(&self, folder: &str) -> Result<std::path::PathBuf, CreateError> {
        let not_found = || CreateError::ImportNotFound(folder.to_owned());
        let name = SafeRelPath::parse(folder).map_err(|_| not_found())?;
        if name.is_root() || name.as_str().contains('/') {
            return Err(not_found());
        }
        let path = self.paths.imports.join(name.as_str());
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => Ok(path),
            _ => Err(not_found()),
        }
    }

    fn import_job(
        &self,
        source: std::path::PathBuf,
        instance: Instance,
        server: MinecraftServer,
        jar: Option<String>,
    ) -> Result<JobId, CreateError> {
        let spec = JobSpec {
            kind: "instance.import",
            title: format!("Importing {}", instance.definition.name),
            instance_id: Some(instance.id()),
        };
        let service = self.clone();
        Ok(self.jobs.spawn(spec, move |job| async move {
            job.step(format!("Copying {}", source.display()));
            let target = instance.layout.server_dir();
            let cancel = job.cancel_token().clone();
            let to = target.clone();
            let summary = tokio::task::spawn_blocking(move || copy_server_dir(&source, &to, &[], &cancel))
                .await
                .map_err(|e| JobFailure::msg(format!("the copy task failed: {e}")))??;
            job.log(format!(
                "Copied {} files ({} bytes)",
                summary.files, summary.bytes
            ));

            job.step("Preparing the server");
            let java_major = instance.definition.java.major;
            service.java.ensure(java_major, &job).await?;
            let port = instance.definition.ports.game;
            let launch = finish_existing(
                server.loader,
                &target,
                jar.as_deref(),
                server.eula_accepted_at,
                port,
                &job,
            )
            .await?;
            instance.layout.assign_ownership(instance.definition.sandbox)?;
            service
                .registry
                .update(instance.id(), |definition| {
                    definition.launch = launch;
                    definition.installed_at = Some(Utc::now());
                })
                .await
                .map_err(JobFailure::from)?;
            job.log("Import complete");
            Ok(())
        })?)
    }
}
