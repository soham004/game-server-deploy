//! Holding world saving still while a running server is backed up.
//!
//! [`pause_saving`] turns automatic saving off, asks the game to write everything to disk and
//! waits until it reports the save ([`GameSignal::Saved`]). The files then stay put until
//! [`SavingPaused::resume`] turns saving back on. If the guard is dropped without `resume` (a
//! panic, or the job's future being dropped) it still sends the resume command, from a task.

use std::{future::Future, time::Duration};

use tokio::sync::broadcast::{self, error::RecvError};

use crate::{
    events::{Event, EventBus, EventKind},
    game::{GameSignal, SaveCommands},
    ids::InstanceId,
    supervisor::{ServerState, SupervisorError, SupervisorHandle},
};

/// Shown in the console next to the commands, so players and admins see where they came from.
pub const AUTHOR: &str = "backup";

/// Sends console lines to a running server: the supervisor, or a fake in tests.
pub trait Console: Clone + Send + Sync + 'static {
    fn send(&self, line: &str) -> impl Future<Output = Result<(), SupervisorError>> + Send;
}

impl Console for SupervisorHandle {
    fn send(&self, line: &str) -> impl Future<Output = Result<(), SupervisorError>> + Send {
        self.send_line(line, Some(AUTHOR.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QuiesceError {
    #[error("could not send {command:?} to the server")]
    Send {
        command: &'static str,
        #[source]
        source: SupervisorError,
    },
    #[error("the server didn't confirm the save within {} seconds", .0.as_secs())]
    Timeout(Duration),
    #[error("the server stopped before the save finished")]
    ServerStopped,
}

/// Saving is off and the world is on disk. Call [`SavingPaused::resume`] when done.
#[derive(Debug)]
#[must_use = "saving stays off until resume() is called"]
pub struct SavingPaused<C: Console> {
    console: Option<C>,
    resume_command: &'static str,
}

/// Turns automatic saving off, flushes the world to disk and waits (up to `timeout`) for the
/// game to confirm. On failure saving is turned back on before returning.
pub async fn pause_saving<C: Console>(
    console: C,
    events: &EventBus,
    instance: InstanceId,
    commands: SaveCommands,
    timeout: Duration,
) -> Result<SavingPaused<C>, QuiesceError> {
    // Subscribe before asking, so the confirmation can't slip past.
    let mut events = events.subscribe();
    console
        .send(commands.pause)
        .await
        .map_err(|source| QuiesceError::Send {
            command: commands.pause,
            source,
        })?;
    let paused = SavingPaused {
        console: Some(console.clone()),
        resume_command: commands.resume,
    };
    let flushed = async {
        console
            .send(commands.flush)
            .await
            .map_err(|source| QuiesceError::Send {
                command: commands.flush,
                source,
            })?;
        wait_for_save(&mut events, instance, timeout).await
    };
    match flushed.await {
        Ok(()) => Ok(paused),
        Err(error) => {
            let _ = paused.resume().await;
            Err(error)
        }
    }
}

impl<C: Console> SavingPaused<C> {
    /// Turns automatic saving back on.
    pub async fn resume(mut self) -> Result<(), SupervisorError> {
        match self.console.take() {
            Some(console) => console.send(self.resume_command).await,
            None => Ok(()),
        }
    }
}

impl<C: Console> Drop for SavingPaused<C> {
    fn drop(&mut self) {
        let Some(console) = self.console.take() else {
            return;
        };
        let command = self.resume_command;
        tracing::warn!("a backup ended without turning saving back on; sending {command:?}");
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = console.send(command).await {
                    tracing::error!(%error, "could not turn saving back on");
                }
            });
        }
    }
}

/// Waits for this instance's "saved" signal; fails early if the server stops meanwhile.
pub async fn wait_for_save(
    events: &mut broadcast::Receiver<Event>,
    instance: InstanceId,
    timeout: Duration,
) -> Result<(), QuiesceError> {
    let wait = async {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                // Missed some events under heavy output; the signal may still come.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return Err(QuiesceError::ServerStopped),
            };
            if event.instance_id != Some(instance) {
                continue;
            }
            match event.kind {
                EventKind::Signal(GameSignal::Saved) => return Ok(()),
                EventKind::State(ServerState::Stopped { .. } | ServerState::Crashed { .. }) => {
                    return Err(QuiesceError::ServerStopped);
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .map_err(|_| QuiesceError::Timeout(timeout))?
}

#[cfg(test)]
mod tests;
