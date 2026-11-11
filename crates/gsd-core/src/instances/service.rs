//! Instance lifecycle: start, stop, restart, kill, console commands, delete.
//!
//! Creation is game-specific (versions, installers) and lives in the game adapter crate; it
//! registers the instance here through [`InstanceRegistry::create`].

use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{
    game::GameAdapter,
    ids::InstanceId,
    instances::{
        Instance, InstanceRegistry, SettingsChange, SettingsError,
        definition::RuntimeChoice,
        launch::{build_process, restart_policy},
        registry::RegistryError,
    },
    java::{JavaRuntime, JavaRuntimes},
    jobs::{InstanceHold, JobRunner},
    supervisor::{
        CommandError, SandboxUnusable, ServerState, Snapshot, SupervisorError, SupervisorHandle, Supervisors,
    },
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
    /// The startup check found that the sandbox can't work here (the text says why and how to fix it).
    #[error(transparent)]
    SandboxUnusable(#[from] SandboxUnusable),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error("no Java runtime with id {0:?}")]
    UnknownRuntime(String),
    #[error("only owners and admins may pick a Java runtime that the app did not install")]
    CustomRuntimeNotAllowed,
    #[error("this server needs Java {needed} or newer; that runtime is Java {found}")]
    RuntimeTooOld { needed: u32, found: u32 },
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
        self.start_as(id, Caller::Request).await
    }

    /// Starts the server from a job that holds the instance's lock (a restore that found it
    /// running). Everyone else gets [`InstanceError::Busy`] while a job runs.
    pub async fn start_from_job(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.start_as(id, Caller::LockHolder).await
    }

    /// Stops the server (or cancels a pending crash restart) and waits until no process is left.
    /// Returns whether it was running or starting, so the caller can start it again.
    pub async fn stop_and_wait(&self, id: InstanceId) -> Result<bool, InstanceError> {
        let instance = self.get(id).await?;
        let Some(handle) = self.supervisors.get(id) else {
            return Ok(false);
        };
        let state = handle.state();
        let was_running = matches!(state, ServerState::Running { .. } | ServerState::Starting { .. });
        if state.is_settled() {
            return Ok(false);
        }
        // Also when it is stopping already: that may be a restart's stop, and this one cancels
        // the start that would follow. It may have stopped by itself since the look above.
        match handle.stop().await {
            Ok(()) | Err(SupervisorError::Command(CommandError::NotRunning)) => {}
            Err(error) => return Err(error.into()),
        }
        // The supervisor kills the process after the stop timeout; allow for that and the exit.
        let limit = Duration::from_secs(instance.definition.behavior.stop_timeout_secs + 30);
        if !handle.wait_for(limit, ServerState::is_settled).await {
            return Err(InstanceError::StillRunning);
        }
        Ok(was_running)
    }

    /// For a job that changes the server's files and holds the instance's lock: the server must
    /// be stopped, and stays so. A crash's pending restart is cancelled (it would start the
    /// server in the middle of the job); a running server is refused. While the job holds the
    /// lock nobody else can start it.
    pub async fn ensure_stopped(&self, id: InstanceId) -> Result<(), InstanceError> {
        let Some(handle) = self.supervisors.get(id) else {
            return Ok(());
        };
        if handle.state().restart_pending() {
            match handle.stop().await {
                Ok(()) | Err(SupervisorError::Command(CommandError::NotRunning)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if handle.state().is_settled() {
            Ok(())
        } else {
            Err(InstanceError::StillRunning)
        }
    }

    /// The instance's lock for something that isn't a job (see [`JobRunner::hold_instance`]).
    pub fn hold(&self, id: InstanceId) -> Result<InstanceHold, InstanceError> {
        self.jobs.hold_instance(id).map_err(|_| InstanceError::Busy)
    }

    /// The adapter of the instance's game.
    pub fn adapter(&self, instance: &Instance) -> Result<Arc<dyn GameAdapter>, InstanceError> {
        self.adapters
            .get(&instance.definition.game)
            .cloned()
            .ok_or_else(|| InstanceError::UnknownGame(instance.definition.game.clone()))
    }

    /// What every start does first: the server gets its control port, and its game sets that channel up.
    async fn prepare_control(&self, id: InstanceId) -> Result<Instance, InstanceError> {
        let instance = self.registry.ensure_control_port(id).await?;
        if let Err(error) = self.adapter(&instance)?.prepare_start(&instance).await {
            tracing::warn!(%id, %error, "could not set up the server's control channel");
        }
        Ok(instance)
    }

    async fn start_as(&self, id: InstanceId, caller: Caller) -> Result<(), InstanceError> {
        let _hold = self.hold_unless_held(id, caller)?;
        let (_, handle) = self.prepare(id).await?;
        let instance = self.prepare_control(id).await?;
        let java = self.resolve_java(&instance).await?;
        let process = build_process(&instance.definition, &instance.layout, &java);
        handle
            .start(process, restart_policy(&instance.definition.behavior))
            .await?;
        Ok(())
    }

    pub async fn restart(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.restart_as(id, Caller::Request).await
    }

    /// Restarts the server from a job that holds the instance's lock (a schedule).
    pub async fn restart_from_job(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.restart_as(id, Caller::LockHolder).await
    }

    async fn restart_as(&self, id: InstanceId, caller: Caller) -> Result<(), InstanceError> {
        let _hold = self.hold_unless_held(id, caller)?;
        let (_, handle) = self.prepare(id).await?;
        let instance = self.prepare_control(id).await?;
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

    /// Who is online now (nobody while the server isn't running).
    pub async fn players(&self, id: InstanceId) -> Result<Vec<String>, InstanceError> {
        match self.supervisors.get(id) {
            Some(handle) => Ok(handle.players().await?),
            None => Ok(Vec::new()),
        }
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

    /// Deletes a stopped instance (its folder goes to the trash). It is held meanwhile, so no
    /// job or start can begin on a server that is going away.
    pub async fn delete(&self, id: InstanceId) -> Result<(), InstanceError> {
        self.get(id).await?;
        if self.state(id).is_active() {
            return Err(InstanceError::StillRunning);
        }
        let _hold = self.hold(id)?;
        self.ensure_stopped(id).await?;
        self.supervisors.remove(id);
        if let Some(cgroups) = self.supervisors.host().cgroups() {
            cgroups.remove(&id.to_string());
        }
        self.registry.remove(id).await?;
        Ok(())
    }

    /// Validates and saves a settings change. A specific Java runtime must exist, be at least the
    /// version the server needs, and (unless `allow_custom_runtime`) be one the app manages.
    /// Applies from the next start.
    pub async fn update_settings(
        &self,
        id: InstanceId,
        change: &SettingsChange,
        allow_custom_runtime: bool,
    ) -> Result<Instance, InstanceError> {
        let current = self.get(id).await?;
        if let Some(RuntimeChoice::Runtime { id: runtime_id }) = &change.java_runtime {
            let runtime = self
                .java
                .list(false)
                .await
                .into_iter()
                .find(|runtime| &runtime.id == runtime_id)
                .ok_or_else(|| InstanceError::UnknownRuntime(runtime_id.clone()))?;
            check_runtime(&runtime, current.definition.java.major, allow_custom_runtime)?;
        }
        let mut validated = current.definition.clone();
        change.apply(&mut validated)?;
        Ok(self
            .registry
            .update(id, move |definition| *definition = validated)
            .await?)
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

    /// A request takes the instance's lock for as long as it starts the server. Checking that
    /// no job runs isn't enough: one could begin while the start gets ready (its control port,
    /// its Java), find the server stopped, and change its files as it comes up.
    fn hold_unless_held(
        &self,
        id: InstanceId,
        caller: Caller,
    ) -> Result<Option<InstanceHold>, InstanceError> {
        match caller {
            Caller::Request => self.hold(id).map(Some),
            Caller::LockHolder => Ok(None),
        }
    }

    async fn prepare(&self, id: InstanceId) -> Result<(Instance, SupervisorHandle), InstanceError> {
        let instance = self.get(id).await?;
        if instance.definition.installed_at.is_none() {
            return Err(InstanceError::NotInstalled);
        }
        self.supervisors.host().ensure_usable()?;
        let adapter = self.adapter(&instance)?;
        let policy = restart_policy(&instance.definition.behavior);
        Ok((instance, self.supervisors.get_or_spawn(id, adapter, policy)))
    }

    fn running_handle(&self, id: InstanceId) -> Result<SupervisorHandle, InstanceError> {
        self.supervisors
            .get(id)
            .ok_or(InstanceError::Supervisor(SupervisorError::Command(
                CommandError::NotRunning,
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

/// Who asks to start a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// A user or the app; refused while a job holds the instance.
    Request,
    /// The job that holds the instance's lock.
    LockHolder,
}

/// Whether `runtime` may run a server needing Java `needed`: new enough, and installed by the app
/// unless the user may pick custom runtimes.
fn check_runtime(
    runtime: &JavaRuntime,
    needed: u32,
    allow_custom_runtime: bool,
) -> Result<(), InstanceError> {
    if !runtime.managed && !allow_custom_runtime {
        return Err(InstanceError::CustomRuntimeNotAllowed);
    }
    if runtime.major < needed {
        return Err(InstanceError::RuntimeTooOld {
            needed,
            found: runtime.major,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn runtime(major: u32, managed: bool) -> JavaRuntime {
        JavaRuntime {
            id: format!("java-{major}"),
            home: PathBuf::from("/opt/java"),
            executable: PathBuf::from("/opt/java/bin/java"),
            major,
            version: format!("{major}.0.1"),
            vendor: None,
            managed,
        }
    }

    #[test]
    fn runtimes_must_be_new_enough_and_managed_unless_allowed() {
        assert!(check_runtime(&runtime(25, true), 25, false).is_ok());
        assert!(
            check_runtime(&runtime(26, true), 25, false).is_ok(),
            "newer is fine"
        );
        assert!(matches!(
            check_runtime(&runtime(21, true), 25, true),
            Err(InstanceError::RuntimeTooOld {
                needed: 25,
                found: 21
            })
        ));
        assert!(matches!(
            check_runtime(&runtime(25, false), 25, false),
            Err(InstanceError::CustomRuntimeNotAllowed)
        ));
        assert!(check_runtime(&runtime(25, false), 25, true).is_ok());
    }
}
