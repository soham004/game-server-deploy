//! Running server processes.
//!
//! - [`state`]: the lifecycle as a pure state machine.
//! - [`actor`]: one task per instance that owns the process and applies the state machine.
//! - [`process`]: spawning and killing.
//! - [`sandbox`]: per-server isolation.
//!
//! [`Supervisors`] maps instances to their actor handles.

mod actor;
pub mod process;
pub mod sandbox;
pub mod state;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

pub use actor::{CONSOLE_HISTORY, Snapshot, SupervisorError, SupervisorHandle};
pub use process::ProcessSpec;
pub use sandbox::{IsolationLevel, SandboxSpec, SandboxSupport};
pub use state::{CommandError, ExitInfo, RestartPolicy, ServerState};

use crate::{events::EventBus, game::GameAdapter, ids::InstanceId};

/// All supervisors, one per instance (created lazily on first use).
#[derive(Debug, Clone)]
pub struct Supervisors {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    events: EventBus,
    support: SandboxSupport,
    handles: Mutex<HashMap<InstanceId, SupervisorHandle>>,
}

impl Supervisors {
    pub fn new(events: EventBus, support: SandboxSupport) -> Self {
        Self {
            inner: Arc::new(Inner {
                events,
                support,
                handles: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn support(&self) -> SandboxSupport {
        self.inner.support
    }

    /// The instance's supervisor, starting one if needed.
    pub fn get_or_spawn(
        &self,
        id: InstanceId,
        adapter: Arc<dyn GameAdapter>,
        policy: RestartPolicy,
    ) -> SupervisorHandle {
        let mut handles = self.handles();
        handles
            .entry(id)
            .or_insert_with(|| {
                SupervisorHandle::spawn(id, adapter, self.inner.events.clone(), self.inner.support, policy)
            })
            .clone()
    }

    pub fn get(&self, id: InstanceId) -> Option<SupervisorHandle> {
        self.handles().get(&id).cloned()
    }

    /// The instance's state, or "stopped" if it never had a supervisor.
    pub fn state(&self, id: InstanceId) -> ServerState {
        self.get(id)
            .map_or_else(ServerState::initial, |handle| handle.state())
    }

    /// Forgets the supervisor; once the last handle is dropped its actor kills any process.
    pub fn remove(&self, id: InstanceId) {
        self.handles().remove(&id);
    }

    /// Gracefully stops every running server, killing those that don't stop within `timeout`.
    pub async fn stop_all(&self, timeout: Duration) {
        let handles: Vec<SupervisorHandle> = self.handles().values().cloned().collect();
        let stops = handles
            .into_iter()
            .filter(|handle| handle.state().is_active())
            .map(|handle| async move {
                let _ = handle.stop().await;
                if !handle.wait_for(timeout, |state| !state.is_active()).await {
                    let _ = handle.kill().await;
                    handle
                        .wait_for(Duration::from_secs(5), |state| !state.is_active())
                        .await;
                }
            });
        futures_util::future::join_all(stops).await;
    }

    fn handles(&self) -> std::sync::MutexGuard<'_, HashMap<InstanceId, SupervisorHandle>> {
        self.inner
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        events::EventKind,
        game::{Diagnostic, GameSignal, LineSource, LogLevel, LogLine, ParsedLine, Severity},
    };

    /// Understands the fake server's output: "READY", "JOIN <name>", "LEAVE <name>", "BIND FAIL".
    #[derive(Debug)]
    struct FakeGame;

    impl GameAdapter for FakeGame {
        fn parse_line(&self, raw: &str) -> ParsedLine {
            let level = if raw.starts_with("ERR") {
                LogLevel::Error
            } else {
                LogLevel::Info
            };
            ParsedLine {
                level,
                thread: None,
                text: raw.to_owned(),
            }
        }

        fn detect_signal(&self, line: &LogLine) -> Option<GameSignal> {
            let text = line.text.as_str();
            if text == "READY" {
                return Some(GameSignal::Ready);
            }
            if let Some(name) = text.strip_prefix("JOIN ") {
                return Some(GameSignal::PlayerJoined { name: name.into() });
            }
            text.strip_prefix("LEAVE ")
                .map(|name| GameSignal::PlayerLeft { name: name.into() })
        }

        fn diagnose(&self, line: &LogLine) -> Option<Diagnostic> {
            (line.text == "BIND FAIL").then(|| Diagnostic {
                code: "port_in_use",
                severity: Severity::Error,
                title: "Port in use".into(),
                detail: String::new(),
            })
        }

        fn stop_command(&self) -> &'static str {
            "stop"
        }
    }

    /// A shell script that behaves like a game server: prints READY, echoes commands, exits on
    /// "stop", crashes on "crash", ignores "stop" when started with `IGNORE_STOP=1`.
    const FAKE_SERVER: &str = r#"
echo "booting"
echo "READY"
while IFS= read -r line; do
  case "$line" in
    stop)  if [ "$IGNORE_STOP" = "1" ]; then echo "ignoring stop"; else echo "stopping"; exit 0; fi ;;
    crash) echo "ERR exploding"; exit 3 ;;
    join*) echo "JOIN ${line#join }" ;;
    leave*) echo "LEAVE ${line#leave }" ;;
    bindfail) echo "BIND FAIL" ;;
    *) echo "echo: $line" ;;
  esac
done
"#;

    fn spec(dir: &std::path::Path, extra_env: &[(&str, &str)]) -> ProcessSpec {
        let mut env = vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())];
        env.extend(extra_env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), FAKE_SERVER.into()],
            cwd: dir.to_owned(),
            env,
            sandbox: None,
        }
    }

    fn policy(auto_restart: bool) -> RestartPolicy {
        RestartPolicy {
            auto_restart,
            crash_limit: 3,
            window: Duration::from_secs(600),
            stop_timeout: Duration::from_millis(500),
        }
    }

    fn setup(auto_restart: bool) -> (Supervisors, SupervisorHandle, EventBus) {
        let events = EventBus::new();
        let supervisors = Supervisors::new(events.clone(), SandboxSupport::none());
        let handle =
            supervisors.get_or_spawn(InstanceId::generate(), Arc::new(FakeGame), policy(auto_restart));
        (supervisors, handle, events)
    }

    const WAIT: Duration = Duration::from_secs(10);

    async fn console_contains(handle: &SupervisorHandle, needle: &str) -> bool {
        for _ in 0..100 {
            let snapshot = handle.snapshot().await.unwrap();
            if snapshot.console.iter().any(|line| line.text.contains(needle)) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn starts_becomes_ready_and_stops_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);

        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
                .await,
            "{:?}",
            handle.state()
        );
        assert_eq!(
            handle.start(spec(dir.path(), &[]), policy(true)).await,
            Err(CommandError::AlreadyRunning.into())
        );

        handle.stop().await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await
        );
        let snapshot = handle.snapshot().await.unwrap();
        let texts: Vec<&str> = snapshot.console.iter().map(|l| l.text.as_str()).collect();
        let stopping = texts
            .iter()
            .position(|t| *t == "stopping")
            .expect("server printed stopping");
        let exited = texts
            .iter()
            .position(|t| t.starts_with("Server process exited"))
            .unwrap();
        assert!(
            stopping < exited,
            "output arrives before the exit notice: {texts:?}"
        );
        assert!(
            snapshot.console.windows(2).all(|w| w[0].seq < w[1].seq),
            "sequence numbers increase"
        );
    }

    #[tokio::test]
    async fn commands_reach_the_server_and_are_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        assert_eq!(
            handle.send_line("hello", None).await,
            Err(SupervisorError::NotAcceptingInput)
        );

        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;
        handle
            .send_line("/say hello", Some("alice".into()))
            .await
            .unwrap();

        assert!(console_contains(&handle, "echo: say hello").await);
        let snapshot = handle.snapshot().await.unwrap();
        let echo = snapshot
            .console
            .iter()
            .find(|line| line.source == LineSource::Input)
            .unwrap();
        assert_eq!(echo.text, "[alice] > say hello");
        assert!(handle.send_line("two\nlines", None).await.is_err());
        handle.kill().await.unwrap();
    }

    #[tokio::test]
    async fn tracks_players_and_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, events) = setup(true);
        let mut receiver = events.subscribe();
        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;

        for command in ["join Steve", "join Alex", "leave Steve", "bindfail", "bindfail"] {
            handle.send_line(command, None).await.unwrap();
        }
        assert!(console_contains(&handle, "LEAVE Steve").await);
        assert!(console_contains(&handle, "BIND FAIL").await);
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(snapshot.players, vec!["Alex".to_owned()]);
        assert_eq!(
            snapshot.diagnostics.len(),
            1,
            "duplicate diagnostics are collapsed"
        );

        let mut saw_join = false;
        while let Ok(event) = receiver.try_recv() {
            if matches!(event.kind, EventKind::Signal(GameSignal::PlayerJoined { .. })) {
                saw_join = true;
            }
        }
        assert!(saw_join);
        handle.kill().await.unwrap();
    }

    #[tokio::test]
    async fn crashes_trigger_an_automatic_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;

        handle.send_line("crash", None).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(
                    s,
                    ServerState::Crashed {
                        restart_at: Some(_),
                        ..
                    }
                ))
                .await
        );
        assert!(console_contains(&handle, "Restarting in 5s").await);
        // Stopping during the backoff cancels the restart.
        handle.stop().await.unwrap();
        assert!(matches!(handle.state(), ServerState::Stopped { .. }));
    }

    #[tokio::test]
    async fn without_auto_restart_a_crash_stays_crashed() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(false);
        handle.start(spec(dir.path(), &[]), policy(false)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;
        handle.send_line("crash", None).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(
                    s,
                    ServerState::Crashed { restart_at: None, .. }
                ))
                .await
        );
        let ServerState::Crashed { exit, .. } = handle.state() else {
            unreachable!()
        };
        assert_eq!(exit.code, Some(3));
    }

    #[tokio::test]
    async fn unresponsive_servers_are_killed_after_the_stop_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        handle
            .start(spec(dir.path(), &[("IGNORE_STOP", "1")]), policy(true))
            .await
            .unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;

        handle.stop().await.unwrap();
        assert!(console_contains(&handle, "ignoring stop").await);
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await,
            "{:?}",
            handle.state()
        );
    }

    #[tokio::test]
    async fn restart_runs_a_fresh_process() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;
        let first_pid = match handle.state() {
            ServerState::Running { pid, .. } => pid,
            _ => None,
        };

        handle.restart(spec(dir.path(), &[]), policy(true)).await.unwrap();
        assert!(
            handle
                .wait_for(
                    WAIT,
                    |s| matches!(s, ServerState::Running { pid, .. } if *pid != first_pid)
                )
                .await
        );
        handle.kill().await.unwrap();
    }

    #[tokio::test]
    async fn spawn_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        let mut bad = spec(dir.path(), &[]);
        bad.program = "/definitely/not/java".into();
        handle.start(bad, policy(true)).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await
        );
        assert!(console_contains(&handle, "Could not start").await);
    }

    #[tokio::test]
    async fn stop_all_stops_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (supervisors, first, _) = setup(true);
        let second = supervisors.get_or_spawn(InstanceId::generate(), Arc::new(FakeGame), policy(true));
        for handle in [&first, &second] {
            handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
                .await;
        }
        supervisors.stop_all(Duration::from_secs(5)).await;
        assert!(!first.state().is_active() && !second.state().is_active());
    }

    #[tokio::test]
    async fn console_history_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        let mut flood = spec(dir.path(), &[]);
        flood.args = vec![
            "-c".into(),
            "i=0; while [ $i -lt 2500 ]; do echo line $i; i=$((i+1)); done".into(),
        ];
        handle.start(flood, policy(false)).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await
        );
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(snapshot.console.len(), CONSOLE_HISTORY);
        assert!(
            snapshot
                .console
                .last()
                .unwrap()
                .text
                .starts_with("Server process exited")
        );
    }
}
