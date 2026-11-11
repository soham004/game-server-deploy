//! Doing one action of a schedule, inside the run's job (which holds the instance's lock).
//!
//! Restart and stop count down with in-game warnings first; restart and start wait until the
//! server is running, so a following action (a backup) finds it up. Every wait ends early when
//! the job is cancelled.

use std::{sync::Arc, time::Duration};

use super::model::Action;
use crate::{
    backups::{BackupKind, BackupRequest, BackupService},
    game::GameAdapter,
    ids::UserId,
    instances::{Instance, InstanceService},
    jobs::JobContext,
    supervisor::ServerState,
};

/// How long a (re)started server may take to report it is running.
const START_TIMEOUT: Duration = Duration::from_secs(600);

/// Why an action failed, for the run history and the job log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ActionFailed(pub String);

impl ActionFailed {
    fn because(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

/// What actions need: the server, the services that act on it, and who the run acts for.
pub(super) struct ActionContext<'a> {
    pub instances: &'a InstanceService,
    pub backups: &'a BackupService,
    pub instance: &'a Instance,
    pub adapter: Arc<dyn GameAdapter>,
    /// Shown in the console next to the commands it sends, e.g. "schedule Nightly restart".
    pub author: String,
    pub run_as: UserId,
    pub job: &'a JobContext,
}

impl ActionContext<'_> {
    pub(super) async fn perform(&self, action: &Action) -> Result<(), ActionFailed> {
        match action {
            Action::Command { command } => self.send(command.trim().trim_start_matches('/')).await,
            Action::Broadcast { message } => self.broadcast(message.trim()).await,
            Action::Restart { warn_secs } => self.restart(warn_secs).await,
            Action::Stop { warn_secs } => self.stop(warn_secs).await,
            Action::Start => self.start().await,
            Action::Backup { scope, note } => {
                let request = BackupRequest {
                    scope: *scope,
                    kind: BackupKind::Scheduled,
                    note: note.clone(),
                    by: Some(self.run_as),
                };
                let backup = self
                    .backups
                    .create(self.instance, &request, self.job)
                    .await
                    .map_err(|error| ActionFailed(crate::jobs::error_chain(&error)))?;
                self.job.log(format!("Saved backup {}", backup.file_name));
                Ok(())
            }
            Action::Wait { secs } => self.sleep(Duration::from_secs(*secs)).await,
        }
    }

    async fn send(&self, line: &str) -> Result<(), ActionFailed> {
        self.instances
            .send_command(self.instance.id(), line, Some(self.author.clone()))
            .await
            .map_err(ActionFailed::because)
    }

    async fn broadcast(&self, message: &str) -> Result<(), ActionFailed> {
        let command = self
            .adapter
            .broadcast_command(message)
            .ok_or_else(|| ActionFailed("this game can't show messages to players".into()))?;
        self.send(&command).await
    }

    async fn restart(&self, warn_secs: &[u64]) -> Result<(), ActionFailed> {
        if !self.is_running() {
            self.job.log("The server isn't running; nothing to restart.");
            return Ok(());
        }
        self.count_down("Restarting", warn_secs).await?;
        // Someone who stopped the server during the countdown wants it stopped.
        if !self.is_running() {
            self.job
                .log("The server was stopped during the countdown; not starting it again.");
            return Ok(());
        }
        self.instances
            .restart_from_job(self.instance.id())
            .await
            .map_err(ActionFailed::because)?;
        self.wait_until_running().await
    }

    async fn stop(&self, warn_secs: &[u64]) -> Result<(), ActionFailed> {
        if !self.is_running() {
            self.job.log("The server isn't running; nothing to stop.");
            return Ok(());
        }
        self.count_down("Stopping", warn_secs).await?;
        self.instances
            .stop_and_wait(self.instance.id())
            .await
            .map(|_| ())
            .map_err(ActionFailed::because)
    }

    async fn start(&self) -> Result<(), ActionFailed> {
        if self.is_running() {
            self.job.log("The server is already running.");
            return Ok(());
        }
        self.instances
            .start_from_job(self.instance.id())
            .await
            .map_err(ActionFailed::because)?;
        self.wait_until_running().await
    }

    /// "Restarting in 10 minutes" … "Restarting in 10 seconds", then the last wait.
    async fn count_down(&self, verb: &str, warn_secs: &[u64]) -> Result<(), ActionFailed> {
        for (index, secs) in warn_secs.iter().enumerate() {
            let message = format!("{verb} in {}", countdown_words(*secs));
            // A warning that can't be shown (the server stopped meanwhile) doesn't stop the action.
            if let Err(error) = self.broadcast(&message).await {
                self.job.log(format!("Could not warn the players: {error}"));
            }
            let next = warn_secs.get(index + 1).copied().unwrap_or(0);
            self.sleep(Duration::from_secs(secs.saturating_sub(next))).await?;
        }
        Ok(())
    }

    async fn wait_until_running(&self) -> Result<(), ActionFailed> {
        let Some(handle) = self.instances.supervisors().get(self.instance.id()) else {
            return Err(ActionFailed("the server has no process".into()));
        };
        // A restart goes from stopping straight to starting, so "stopped" is an end too: the
        // start failed, or someone stopped the server. Waiting on would hold the instance's
        // lock for the whole timeout, and nobody could start it meanwhile.
        let settled = tokio::select! {
            settled = handle.wait_for(START_TIMEOUT, |state| {
                !matches!(state, ServerState::Starting { .. } | ServerState::Stopping { .. })
            }) => settled,
            () = self.job.cancel_token().cancelled() => return Err(ActionFailed("cancelled".into())),
        };
        match handle.state() {
            ServerState::Running { .. } => Ok(()),
            _ if !settled => Err(ActionFailed(
                "the server didn't finish starting within 10 minutes".into(),
            )),
            state => Err(ActionFailed(format!(
                "the server is {} instead of running",
                state.name()
            ))),
        }
    }

    async fn sleep(&self, duration: Duration) -> Result<(), ActionFailed> {
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = self.job.cancel_token().cancelled() => Err(ActionFailed("cancelled".into())),
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.instances.state(self.instance.id()),
            ServerState::Running { .. } | ServerState::Starting { .. }
        )
    }
}

/// "10 minutes", "1 minute", "30 seconds", "1 hour".
fn countdown_words(secs: u64) -> String {
    let unit = |count: u64, name: &str| format!("{count} {name}{}", if count == 1 { "" } else { "s" });
    match secs {
        s if s >= 3_600 && s % 3_600 == 0 => unit(s / 3_600, "hour"),
        s if s >= 60 && s % 60 == 0 => unit(s / 60, "minute"),
        s => unit(s, "second"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdowns_read_naturally() {
        assert_eq!(countdown_words(600), "10 minutes");
        assert_eq!(countdown_words(60), "1 minute");
        assert_eq!(countdown_words(3_600), "1 hour");
        assert_eq!(countdown_words(90), "90 seconds");
        assert_eq!(countdown_words(1), "1 second");
    }
}
