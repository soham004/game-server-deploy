//! Instance lifecycle: start, stop, restart, kill, console commands, delete.
//!
//! Creation is game-specific (versions, installers) and lives in the game adapter crate; it
//! registers the instance here through [`InstanceRegistry::create`].

use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{
    game::GameAdapter,
    ids::InstanceId,
    instances::{
        Instance, InstanceRegistry,
        definition::RuntimeChoice,
        launch::{build_process, restart_policy},
        registry::RegistryError,
    },
    java::{JavaRuntime, JavaRuntimes},
    jobs::JobRunner,
    supervisor::{ServerState, Snapshot, SupervisorError, SupervisorHandle, Supervisors},
};

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("instance not found")]
    NotFound,
    #[error("an operation (such as an install) is still running on this instance")]
    Busy,
    #[error("no adapter for game {0:?}")]
    UnknownGame(String),
    #[error("Java {major} is not installed")]
    JavaMissing { major: u32 },
    #[error("stop the server first")]
    StillRunning,
    #[error("the server files are not installed yet")]
    NotInstalled,
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

#[derive(Debug, Clone)]
pub struct InstanceService {
    registry: InstanceRegistry,
    supervisors: Supervisors,
    java: JavaRuntimes,
    jobs: JobRunner,
    adapters: Arc<HashMap<String, Arc<dyn GameAdapter>>>,
}

impl InstanceService {
    pub fn new(
        registry: InstanceRegistry,
        supervisors: Supervisors,
        java: JavaRuntimes,
        jobs: JobRunner,
        adapters: HashMap<String, Arc<dyn GameAdapter>>,
    ) -> Self {
        Self {
            registry,
            supervisors,
            java,
            jobs,
            adapters: Arc::new(adapters),
        }
    }

    pub fn registry(&self) -> &InstanceRegistry {
        &self.registry
    }

    pub fn supervisors(&self) -> &Supervisors {
        &self.supervisors
    }

    pub async fn get(&self, id: InstanceId) -> Result<Instance, InstanceError> {
        self.registry.get(id).await.ok_or(InstanceError::NotFound)
    }

    pub fn state(&self, id: InstanceId) -> ServerState {
        self.supervisors.state(id)
    }

    /// True while a job (install, …) holds the instance.
    pub fn is_busy(&self, id: InstanceId) -> bool {
        self.jobs.is_instance_busy(id)
    }

    pub async fn start(&self, id: InstanceId) -> Result<(), InstanceError> {
        let (instance, handle) = self.prepare(id).await?;
        let java = self.resolve_java(&instance).await?;
        let process = build_process(&instance.definition, &instance.layout, &java);
        handle
            .start(process, restart_policy(&instance.definition.behavior))
            .await?;
        Ok(())
    }

    pub async fn restart(&self, id: InstanceId) -> Result<(), InstanceError> {
        let (instance, handle) = self.prepare(id).await?;
        let java = self.resolve_java(&instance).await?;
        let process = build_process(&instance.definition, &instance.layout, &java);
        handle
            .restart(process, restart_policy(&instance.definition.behavior))
            .await?;
        Ok(())
    }

    pub async fn stop(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.get(id).await?;
        self.running_handle(id)?.stop().await?;
        Ok(())
    }

    pub async fn kill(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.get(id).await?;
        self.running_handle(id)?.kill().await?;
        Ok(())
    }

    pub async fn send_command(
        &self,
        id: InstanceId,
        line: &str,
        author: Option<String>,
    ) -> Result<(), InstanceError> {
        self.get(id).await?;
        self.running_handle(id)?.send_line(line, author).await?;
        Ok(())
    }

    /// Runtime view; a stopped instance that never ran has an empty console.
    pub async fn snapshot(&self, id: InstanceId) -> Result<Snapshot, InstanceError> {
        self.get(id).await?;
        match self.supervisors.get(id) {
            Some(handle) => Ok(handle.snapshot().await?),
            None => Ok(Snapshot {
                state: ServerState::initial(),
                players: vec![],
                diagnostics: vec![],
                console: vec![],
            }),
        }
    }

    /// Deletes a stopped instance (its folder goes to the trash).
    pub async fn delete(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.get(id).await?;
        if self.state(id).is_active() {
            return Err(InstanceError::StillRunning);
        }
        if self.is_busy(id) {
            return Err(InstanceError::Busy);
        }
        self.supervisors.remove(id);
        self.registry.remove(id).await?;
        Ok(())
    }

    /// Starts every instance marked `auto_start` (called once at app startup).
    pub async fn start_auto_start_instances(&self) {
        for instance in self.registry.list().await {
            if instance.definition.behavior.auto_start
                && let Err(error) = self.start(instance.id()).await
            {
                tracing::warn!(instance = %instance.definition.name, %error, "auto-start failed");
            }
        }
    }

    /// Stops every running server (app shutdown).
    pub async fn stop_all(&self, timeout: Duration) {
        self.supervisors.stop_all(timeout).await;
    }

    async fn prepare(&self, id: InstanceId) -> Result<(Instance, SupervisorHandle), InstanceError> {
        let instance = self.get(id).await?;
        if self.is_busy(id) {
            return Err(InstanceError::Busy);
        }
        if instance.definition.installed_at.is_none() {
            return Err(InstanceError::NotInstalled);
        }
        let adapter = self
            .adapters
            .get(&instance.definition.game)
            .cloned()
            .ok_or_else(|| InstanceError::UnknownGame(instance.definition.game.clone()))?;
        let policy = restart_policy(&instance.definition.behavior);
        Ok((instance, self.supervisors.get_or_spawn(id, adapter, policy)))
    }

    fn running_handle(&self, id: InstanceId) -> Result<SupervisorHandle, InstanceError> {
        self.supervisors
            .get(id)
            .ok_or(InstanceError::Supervisor(SupervisorError::Command(
                crate::supervisor::CommandError::NotRunning,
            )))
    }

    async fn resolve_java(&self, instance: &Instance) -> Result<JavaRuntime, InstanceError> {
        let settings = &instance.definition.java;
        let found = match &settings.runtime {
            RuntimeChoice::Auto => self.java.find(settings.major).await,
            RuntimeChoice::Runtime { id } => self
                .java
                .list(false)
                .await
                .into_iter()
                .find(|runtime| &runtime.id == id),
        };
        found.ok_or(InstanceError::JavaMissing {
            major: settings.major,
        })
    }
}
