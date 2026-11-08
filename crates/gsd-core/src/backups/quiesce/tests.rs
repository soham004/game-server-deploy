#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use super::*;
use crate::supervisor::CommandError;

const COMMANDS: SaveCommands = SaveCommands {
    pause: "save-off",
    flush: "save-all flush",
    resume: "save-on",
};
const SHORT: Duration = Duration::from_millis(200);

/// What the fake server does when asked to flush.
#[derive(Debug, Clone, Copy)]
enum OnFlush {
    /// Reports the save, like a healthy server.
    Confirm,
    /// Reports a save of a *different* server, then nothing.
    ConfirmOtherInstance,
    /// Never answers.
    Silent,
    /// Stops.
    Stop,
}

#[derive(Debug, Clone)]
struct FakeConsole {
    sent: Arc<Mutex<Vec<String>>>,
    events: EventBus,
    instance: InstanceId,
    on_flush: OnFlush,
    running: bool,
}

impl FakeConsole {
    fn new(events: &EventBus, instance: InstanceId, on_flush: OnFlush) -> Self {
        Self {
            sent: Arc::default(),
            events: events.clone(),
            instance,
            on_flush,
            running: true,
        }
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Console for FakeConsole {
    async fn send(&self, line: &str) -> Result<(), SupervisorError> {
        if !self.running {
            return Err(SupervisorError::Command(CommandError::NotRunning));
        }
        self.sent.lock().unwrap().push(line.to_owned());
        if line == COMMANDS.flush {
            match self.on_flush {
                OnFlush::Confirm => self
                    .events
                    .publish(Some(self.instance), EventKind::Signal(GameSignal::Saved)),
                OnFlush::ConfirmOtherInstance => self
                    .events
                    .publish(Some(InstanceId::generate()), EventKind::Signal(GameSignal::Saved)),
                OnFlush::Silent => {}
                OnFlush::Stop => self
                    .events
                    .publish(Some(self.instance), EventKind::State(ServerState::initial())),
            }
        }
        Ok(())
    }
}

fn setup(on_flush: OnFlush) -> (FakeConsole, EventBus, InstanceId) {
    let events = EventBus::new();
    let instance = InstanceId::generate();
    (FakeConsole::new(&events, instance, on_flush), events, instance)
}

#[tokio::test]
async fn pauses_flushes_and_waits_for_the_save_then_resumes() {
    let (console, events, instance) = setup(OnFlush::Confirm);

    let paused = pause_saving(console.clone(), &events, instance, COMMANDS, SHORT)
        .await
        .unwrap();
    assert_eq!(
        console.sent(),
        ["save-off", "save-all flush"],
        "saving stays off while paused"
    );

    paused.resume().await.unwrap();
    assert_eq!(console.sent(), ["save-off", "save-all flush", "save-on"]);
}

#[tokio::test]
async fn no_confirmation_times_out_and_saving_is_turned_back_on() {
    for on_flush in [OnFlush::Silent, OnFlush::ConfirmOtherInstance] {
        let (console, events, instance) = setup(on_flush);

        let error = pause_saving(console.clone(), &events, instance, COMMANDS, SHORT)
            .await
            .unwrap_err();

        assert!(matches!(error, QuiesceError::Timeout(_)), "{on_flush:?}: {error}");
        assert_eq!(
            console.sent(),
            ["save-off", "save-all flush", "save-on"],
            "{on_flush:?}"
        );
    }
}

#[tokio::test]
async fn a_server_that_stops_meanwhile_fails_at_once() {
    let (console, events, instance) = setup(OnFlush::Stop);

    let error = pause_saving(
        console.clone(),
        &events,
        instance,
        COMMANDS,
        Duration::from_secs(60),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, QuiesceError::ServerStopped), "{error}");
}

#[tokio::test]
async fn a_server_that_is_not_running_gets_nothing_else() {
    let (mut console, events, instance) = setup(OnFlush::Confirm);
    console.running = false;

    let error = pause_saving(console.clone(), &events, instance, COMMANDS, SHORT)
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            QuiesceError::Send {
                command: "save-off",
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(console.sent(), Vec::<String>::new());
}

#[tokio::test]
async fn dropping_the_guard_still_turns_saving_back_on() {
    let (console, events, instance) = setup(OnFlush::Confirm);

    let paused = pause_saving(console.clone(), &events, instance, COMMANDS, SHORT)
        .await
        .unwrap();
    drop(paused);

    for _ in 0..50 {
        if console.sent().last().map(String::as_str) == Some("save-on") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("save-on was never sent: {:?}", console.sent());
}
