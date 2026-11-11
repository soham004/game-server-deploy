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
pub use sandbox::{HostSandbox, IsolationLevel, SandboxSpec, SandboxSupport, SandboxUnusable};
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
    host: HostSandbox,
    handles: Mutex<HashMap<InstanceId, SupervisorHandle>>,
}

impl Supervisors {
    pub fn new(events: EventBus, host: HostSandbox) -> Self {
        Self {
            inner: Arc::new(Inner {
                events,
                host,
                handles: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// What the sandbox can do here; a problem blocks every start.
    pub fn host(&self) -> &HostSandbox {
        &self.inner.host
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
                SupervisorHandle::spawn(
                    id,
                    adapter,
                    self.inner.events.clone(),
                    self.inner.host.clone(),
                    policy,
                )
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
            let title = match line.text.as_str() {
                "BIND FAIL" => "Couldn't open the port",
                "BIND CAUSE in use" => "Port in use",
                _ => return None,
            };
            Some(Diagnostic {
                code: "port_bind_failed",
                severity: Severity::Error,
                title: title.into(),
                detail: String::new(),
            })
        }

        fn stop_command(&self) -> &'static str {
            "stop"
        }

        fn software(&self, _server: &toml::Table) -> Option<String> {
            None
        }

        fn world_folders(&self, _server_dir: &std::path::Path) -> Vec<String> {
            vec!["world".into()]
        }

        fn backup_excludes(&self) -> &[&str] {
            &["logs"]
        }

        fn save_commands(&self) -> Option<crate::game::SaveCommands> {
            None
        }

        fn broadcast_command(&self, message: &str) -> Option<String> {
            Some(format!("say {message}"))
        }
    }

    /// A shell script that behaves like a game server: prints READY, echoes commands, exits on
    /// "stop", crashes on "crash", ignores "stop" when started with `IGNORE_STOP=1`.
    const FAKE_SERVER: &str = r#"
echo "booting"
if [ "$EXIT_BEFORE_READY" = "1" ]; then echo "Failed to initialize server"; exit 0; fi
if [ "$NO_READY" != "1" ]; then echo "READY"; fi
while IFS= read -r line; do
  case "$line" in
    stop)  if [ "$IGNORE_STOP" = "1" ]; then echo "ignoring stop"; else echo "stopping"; exit 0; fi ;;
    crash) echo "ERR exploding"; exit 3 ;;
    join*) echo "JOIN ${line#join }" ;;
    leave*) echo "LEAVE ${line#leave }" ;;
    bindfail) echo "BIND FAIL" ;;
    bindcause) echo "BIND CAUSE in use" ;;
    hog) head -c 400000000 /dev/zero | tail -c 300000000 > /dev/null ;;
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
        let supervisors = Supervisors::new(events.clone(), HostSandbox::disabled());
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
    async fn a_later_explanation_replaces_a_diagnostic_with_the_same_code() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, events) = setup(true);
        let mut receiver = events.subscribe();
        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;
        for command in ["bindfail", "bindcause", "bindcause"] {
            handle.send_line(command, None).await.unwrap();
        }
        assert!(console_contains(&handle, "BIND CAUSE").await);
        let snapshot = handle.snapshot().await.unwrap();
        let titles: Vec<&str> = snapshot.diagnostics.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, ["Port in use"], "the cause replaced the symptom");

        let mut published = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let EventKind::Diagnostic(diagnostic) = event.kind {
                published.push(diagnostic.title);
            }
        }
        assert_eq!(
            published,
            ["Couldn't open the port", "Port in use"],
            "no repeat event"
        );
        handle.kill().await.unwrap();
    }

    /// Regression (real Forge run): Minecraft exits with code 0 after "Failed to initialize
    /// server". Before ready, that is a crash, not a stop.
    #[tokio::test]
    async fn a_clean_exit_before_ready_counts_as_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(false);
        handle
            .start(spec(dir.path(), &[("EXIT_BEFORE_READY", "1")]), policy(false))
            .await
            .unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Crashed { .. }))
                .await,
            "{:?}",
            handle.state()
        );
    }

    #[tokio::test]
    async fn typing_stop_before_ready_is_still_a_stop() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        handle
            .start(spec(dir.path(), &[("NO_READY", "1")]), policy(true))
            .await
            .unwrap();
        assert!(console_contains(&handle, "booting").await);
        handle.send_line("stop", None).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await,
            "{:?}",
            handle.state()
        );
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

    /// A shell that says READY and then runs `script` (the fake server's command loop isn't
    /// what these need).
    fn script_spec(dir: &std::path::Path, script: &str) -> ProcessSpec {
        let mut process = spec(dir, &[]);
        process.args = vec!["-c".into(), format!("echo READY; {script}")];
        process
    }

    async fn console_line_with(handle: &SupervisorHandle, needle: &str) -> Option<String> {
        if !console_contains(handle, needle).await {
            return None;
        }
        let snapshot = handle.snapshot().await.unwrap();
        snapshot
            .console
            .iter()
            .find(|line| line.text.contains(needle))
            .map(|line| line.text.clone())
    }

    /// Java's console ends a line at a carriage return too, so one would smuggle in a second
    /// command. No control character gets through.
    #[tokio::test]
    async fn a_command_with_any_control_character_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        handle.start(spec(dir.path(), &[]), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;
        for sneaky in [
            "say hi\rjoin Mallory",
            "say hi\njoin Mallory",
            "say\u{0}hi",
            "say\thi",
        ] {
            assert!(
                matches!(
                    handle.send_line(sneaky, None).await,
                    Err(SupervisorError::Write(_))
                ),
                "{sneaky:?}"
            );
        }
        handle.send_line("  /say fine  ", None).await.unwrap();
        assert!(console_contains(&handle, "echo: say fine").await);
        let snapshot = handle.snapshot().await.unwrap();
        assert!(snapshot.players.is_empty(), "nothing after the break was run");
        handle.kill().await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_during_a_restart_leaves_the_server_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(true);
        // It takes a moment to stop, so the stop below arrives while it is still stopping.
        let slow_to_stop = script_spec(dir.path(), "read line; sleep 1; exit 0");
        handle.start(slow_to_stop.clone(), policy(true)).await.unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;

        handle.restart(slow_to_stop, policy(true)).await.unwrap();
        assert!(matches!(handle.state(), ServerState::Stopping { .. }));
        handle.stop().await.unwrap();

        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await,
            "{:?}",
            handle.state()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            matches!(handle.state(), ServerState::Stopped { .. }),
            "it did not come back up: {:?}",
            handle.state()
        );
    }

    /// A hung server stops reading its console. Once the pipe is full a write would wait for ever,
    /// and with it everything else the supervisor does: stop, kill, the stop timer.
    #[tokio::test]
    async fn a_server_that_stopped_reading_its_console_can_still_be_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(false);
        handle
            .start(script_spec(dir.path(), "sleep 600"), policy(false))
            .await
            .unwrap();
        handle
            .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
            .await;

        // More than a pipe holds.
        let unread = "x".repeat(1024 * 1024);
        let sent = tokio::time::timeout(Duration::from_secs(8), handle.send_line(unread, None))
            .await
            .expect("the write gives up instead of waiting");
        assert!(matches!(sent, Err(SupervisorError::Write(_))), "{sent:?}");
        assert!(console_contains(&handle, "isn't reading its console").await);
        assert_eq!(
            handle.send_line("say hi", None).await,
            Err(SupervisorError::NotAcceptingInput),
            "half a line is in the pipe: nothing more is sent after it"
        );

        // Asking nicely isn't possible any more, so the stop goes straight to killing.
        handle.stop().await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await,
            "{:?}",
            handle.state()
        );
    }

    /// "Stopped" has to mean nothing of the server runs: the app changes its files next.
    #[tokio::test]
    async fn what_the_server_started_is_stopped_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (_supervisors, handle, _) = setup(false);
        let leaves_a_child = script_spec(dir.path(), "sleep 300 & echo \"CHILD $!\"; read line; exit 0");
        handle.start(leaves_a_child, policy(false)).await.unwrap();
        let child = console_line_with(&handle, "CHILD ")
            .await
            .expect("the child's pid was printed");
        let pid = child.trim_start_matches("CHILD ").trim().to_owned();
        let alive = || std::path::Path::new("/proc").join(&pid).exists();
        assert!(alive(), "{child}");

        handle.stop().await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Stopped { .. }))
                .await
        );
        for _ in 0..50 {
            if !alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("process {pid} outlived the server that started it");
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
        // It never prints READY, so its exit counts as a failed start; only the history matters here.
        assert!(handle.wait_for(WAIT, |s| !s.is_active()).await);
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

    /// Needs root and cgroup v2 (skipped otherwise): the kernel stops a server that outgrows its
    /// memory ceiling, and the console and problems say so instead of only "signal 9".
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn explains_when_the_memory_ceiling_stopped_the_server() {
        use sandbox::{
            CgroupLimits, CgroupRequest, ResourceLimits, cgroup::tests::kernel::RealTree,
            system_read_only_paths, system_read_write_paths,
        };

        let Some(real) = RealTree::new() else { return };
        let dir = tempfile::tempdir().unwrap();
        let supervisors = Supervisors::new(EventBus::new(), real.host().clone());
        let handle = supervisors.get_or_spawn(InstanceId::generate(), Arc::new(FakeGame), policy(false));
        let mut process = spec(dir.path(), &[]);
        let mut read_write = system_read_write_paths();
        read_write.push(dir.path().to_owned());
        process.sandbox = Some(SandboxSpec {
            user: None,
            read_write,
            read_only: system_read_only_paths(),
            bind_tcp: vec![],
            connect_tcp: vec![],
            limits: ResourceLimits::default(),
            cgroup: Some(CgroupRequest {
                name: "server".into(),
                limits: CgroupLimits {
                    memory_max: 48 * 1024 * 1024,
                    tasks_max: 100,
                },
            }),
        });

        handle.start(process, policy(false)).await.unwrap();
        assert!(
            handle
                .wait_for(WAIT, |s| matches!(s, ServerState::Running { .. }))
                .await
        );
        handle.send_line("hog", None).await.unwrap();
        assert!(
            handle.wait_for(WAIT, |s| !s.is_active()).await,
            "{:?}",
            handle.state()
        );

        assert!(console_contains(&handle, "used all of its 48 MB memory limit").await);
        let snapshot = handle.snapshot().await.unwrap();
        let codes: Vec<&str> = snapshot.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"memory_limit"), "{codes:?}");
    }

    /// Needs root and cgroup v2 (skipped otherwise). A process that left the server's process group
    /// (`setsid`) isn't reached by killing the group; its control group still holds it, and is
    /// emptied when the server exits.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_process_that_left_the_servers_group_is_stopped_through_its_control_group() {
        use sandbox::{
            CgroupLimits, CgroupRequest, ResourceLimits, cgroup::tests::kernel::RealTree,
            system_read_only_paths, system_read_write_paths,
        };

        let Some(real) = RealTree::new() else { return };
        let dir = tempfile::tempdir().unwrap();
        let supervisors = Supervisors::new(EventBus::new(), real.host().clone());
        let handle = supervisors.get_or_spawn(InstanceId::generate(), Arc::new(FakeGame), policy(false));
        let pid_file = dir.path().join("escaped.pid");
        let mut process = script_spec(
            dir.path(),
            &format!(
                "setsid sh -c 'echo $$ > {}; exec sleep 300' & read line; exit 0",
                pid_file.display()
            ),
        );
        let mut read_write = system_read_write_paths();
        read_write.push(dir.path().to_owned());
        process.sandbox = Some(SandboxSpec {
            user: None,
            read_write,
            read_only: system_read_only_paths(),
            bind_tcp: vec![],
            connect_tcp: vec![],
            limits: ResourceLimits::default(),
            cgroup: Some(CgroupRequest {
                name: "escaper".into(),
                limits: CgroupLimits::for_heap(512),
            }),
        });

        handle.start(process, policy(false)).await.unwrap();
        let mut escaped = None;
        for _ in 0..100 {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                && !pid.trim().is_empty()
            {
                escaped = Some(pid.trim().to_owned());
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let escaped = escaped.expect("the escaping process started");
        let alive = || std::path::Path::new("/proc").join(&escaped).exists();
        assert!(alive());

        handle.stop().await.unwrap();
        assert!(handle.wait_for(WAIT, |s| !s.is_active()).await);
        assert!(console_contains(&handle, "left processes running; they were stopped").await);
        for _ in 0..50 {
            if !alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("process {escaped} outlived the server");
    }
}
