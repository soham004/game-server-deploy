//! Cloning a server: a new instance (its own port, sandbox user and folders) with a copy of the
//! source's files, made in a background job. Logs and crash reports are never copied; worlds
//! only on request. The copy's `server.properties` gets the new port.

use chrono::Utc;
use gsd_core::{
    files::copy_server_dir,
    ids::{InstanceId, JobId},
    instances::{
        Instance, NewInstance, definition::Behavior, registry::RegistryError, settings::MAX_NAME_CHARS,
    },
    jobs::{JobFailure, JobSpec},
};

use super::{CreateError, MinecraftService};
use crate::{install::point_at_port, properties::PropertiesDoc, settings::MinecraftServer};

/// Folders never worth copying.
const NEVER_COPIED: [&str; 2] = ["logs", "crash-reports"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRequest {
    pub name: String,
    /// Copy the world folders too (otherwise the clone generates a new world).
    pub include_worlds: bool,
}

impl MinecraftService {
    /// Registers the clone and starts the copy job. The caller makes sure the source is stopped
    /// (a running server's world is mid-write).
    pub async fn clone_instance(
        &self,
        source_id: InstanceId,
        request: CloneRequest,
    ) -> Result<(Instance, JobId), CreateError> {
        let source = self
            .registry
            .get(source_id)
            .await
            .ok_or(CreateError::Registry(RegistryError::NotFound))?;
        if source.definition.installed_at.is_none() {
            return Err(CreateError::SourceNotInstalled);
        }
        MinecraftServer::from_table(&source.definition.server)?;
        let name = request.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(CreateError::InvalidName);
        }

        let definition = &source.definition;
        let clone = self
            .registry
            .create(NewInstance {
                name: name.to_owned(),
                game: definition.game.clone(),
                server: definition.server.clone(),
                launch: definition.launch.clone(),
                java: definition.java.clone(),
                behavior: Behavior {
                    auto_start: false,
                    ..definition.behavior.clone()
                },
            })
            .await?;
        let job = self.copy_job(source, clone.clone(), request.include_worlds)?;
        Ok((clone, job))
    }

    fn copy_job(
        &self,
        source: Instance,
        clone: Instance,
        include_worlds: bool,
    ) -> Result<JobId, CreateError> {
        let spec = JobSpec {
            kind: "instance.clone",
            title: format!("Copying {} to {}", source.definition.name, clone.definition.name),
            instance_id: Some(clone.id()),
        };
        let registry = self.registry.clone();
        Ok(self.jobs.spawn(spec, move |job| async move {
            let skip = skipped_folders(&source, include_worlds);
            job.step("Copying files");
            job.log(format!("Leaving out: {}", skip.join(", ")));
            let (from, to) = (source.layout.server_dir(), clone.layout.server_dir());
            let cancel = job.cancel_token().clone();
            let summary = tokio::task::spawn_blocking(move || copy_server_dir(&from, &to, &skip, &cancel))
                .await
                .map_err(|e| JobFailure::msg(format!("the copy task failed: {e}")))??;
            job.log(format!(
                "Copied {} files ({} bytes); skipped {} links or special files",
                summary.files, summary.bytes, summary.skipped
            ));

            job.step("Giving the copy its own port");
            point_at_port(&clone.layout.server_dir(), clone.definition.ports.game)?;
            clone.layout.assign_ownership(clone.definition.sandbox)?;
            registry
                .update(clone.id(), |definition| {
                    definition.installed_at = Some(Utc::now());
                })
                .await
                .map_err(JobFailure::from)?;
            Ok(())
        })?)
    }
}

/// Logs and crash reports always; the worlds (`level-name` and its nether/end folders) unless
/// asked to copy them.
fn skipped_folders(source: &Instance, include_worlds: bool) -> Vec<String> {
    let mut skip: Vec<String> = NEVER_COPIED.iter().map(|name| (*name).to_owned()).collect();
    if !include_worlds {
        let properties =
            std::fs::read(source.layout.server_dir().join("server.properties")).unwrap_or_default();
        let world = PropertiesDoc::from_bytes(&properties)
            .get("level-name")
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .unwrap_or("world")
            .to_owned();
        skip.extend([format!("{world}_nether"), format!("{world}_the_end"), world]);
    }
    skip
}
