//! The server lifecycle as a pure state machine.
//!
//! `Machine::handle(input)` returns the new state plus the side effects the actor must perform
//! (spawn, send the stop command, kill, schedule a restart). No IO happens here, so every rule
//! below is unit-tested directly.
//!
//! ```text
//! Stopped/Crashed ──Start──▶ Starting ──Ready──▶ Running
//! Starting/Running ──Stop──▶ Stopping ──Exited──▶ Stopped        (──StopTimedOut──▶ Kill)
//! Starting/Running ──Exited(code≠0)──▶ Crashed ──RestartDue──▶ Starting   (auto-restart, with backoff)
//! Crashed ──(too many crashes in the window)──▶ Crashed, no restart, CrashLoop effect
//! ```

use std::{collections::VecDeque, time::Duration};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// The lifecycle state shown to users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ServerState {
    Stopped {
        since: Option<DateTime<Utc>>,
        exit: Option<ExitInfo>,
    },
    Starting {
        since: DateTime<Utc>,
        pid: Option<u32>,
    },
    Running {
        since: DateTime<Utc>,
        pid: Option<u32>,
    },
    Stopping {
        since: DateTime<Utc>,
        pid: Option<u32>,
    },
    Crashed {
        at: DateTime<Utc>,
        exit: ExitInfo,
        /// When the automatic restart will happen; `None` when it won't.
        restart_at: Option<DateTime<Utc>>,
    },
}

impl ServerState {
    pub fn initial() -> Self {
        Self::Stopped {
            since: None,
            exit: None,
        }
    }

    /// A process exists (or is being started).
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Starting { .. } | Self::Running { .. } | Self::Stopping { .. }
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Stopped { .. } => "stopped",
            Self::Starting { .. } => "starting",
            Self::Running { .. } => "running",
            Self::Stopping { .. } => "stopping",
            Self::Crashed { .. } => "crashed",
        }
    }

    fn pid(&self) -> Option<u32> {
        match self {
            Self::Starting { pid, .. } | Self::Running { pid, .. } | Self::Stopping { pid, .. } => *pid,
            _ => None,
        }
    }
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExitInfo {
    pub code: Option<i32>,
    /// Unix signal that killed the process, if any.
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Auto-restart rules (from the instance's `behavior`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    pub auto_restart: bool,
    /// Restarts allowed within `window` before giving up.
    pub crash_limit: u32,
    pub window: Duration,
    pub stop_timeout: Duration,
}

/// Something that happened, from a user, the process, or a timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Start,
    Stop,
    Restart,
    Kill,
    Spawned {
        pid: Option<u32>,
    },
    SpawnFailed,
    Ready,
    /// Someone sent the game's stop command through the console: the coming exit is expected.
    StopTyped,
    Exited(ExitInfo),
    StopTimedOut,
    RestartDue,
}

/// Work the actor must do after a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn,
    SendStopCommand,
    StartStopTimer(Duration),
    Kill,
    ScheduleRestart(Duration),
    CancelRestart,
    /// Too many crashes: auto-restart gave up.
    CrashLoop {
        crashes: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("the server is already running")]
    AlreadyRunning,
    #[error("the server is not running")]
    NotRunning,
}

/// Delays before successive automatic restarts.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(45),
    Duration::from_secs(120),
];

#[derive(Debug)]
pub struct Machine {
    state: ServerState,
    policy: RestartPolicy,
    crashes: VecDeque<DateTime<Utc>>,
    /// A stop was requested (by a user or the restart flow), so the next exit is expected.
    stop_requested: bool,
    /// Start again once the current process has exited.
    restart_after_stop: bool,
}

impl Machine {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            state: ServerState::initial(),
            policy,
            crashes: VecDeque::new(),
            stop_requested: false,
            restart_after_stop: false,
        }
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    pub fn set_policy(&mut self, policy: RestartPolicy) {
        self.policy = policy;
    }

    /// Applies one input at time `now`. User commands that make no sense in the current state
    /// return an error; process and timer inputs that arrive late are ignored.
    pub fn handle(&mut self, input: Input, now: DateTime<Utc>) -> Result<Vec<Effect>, CommandError> {
        match input {
            Input::Start => self.start(now),
            Input::Stop => self.stop(now, false),
            Input::Restart => self.restart(now),
            Input::Kill => self.kill(now),
            Input::Spawned { pid } => Ok(self.spawned(pid)),
            Input::SpawnFailed => Ok(self.spawn_failed(now)),
            Input::Ready => Ok(self.ready(now)),
            Input::StopTyped => {
                if self.state.is_active() {
                    self.stop_requested = true;
                }
                Ok(vec![])
            }
            Input::Exited(exit) => Ok(self.exited(exit, now)),
            Input::StopTimedOut => Ok(self.stop_timed_out()),
            Input::RestartDue => Ok(self.restart_due(now)),
        }
    }

    fn start(&mut self, now: DateTime<Utc>) -> Result<Vec<Effect>, CommandError> {
        let pending_restart = matches!(
            self.state,
            ServerState::Crashed {
                restart_at: Some(_),
                ..
            }
        );
        match self.state {
            ServerState::Stopped { .. } | ServerState::Crashed { .. } => {
                self.begin_start(now);
                let mut effects = vec![Effect::Spawn];
                if pending_restart {
                    effects.insert(0, Effect::CancelRestart);
                }
                Ok(effects)
            }
            _ => Err(CommandError::AlreadyRunning),
        }
    }

    fn stop(&mut self, now: DateTime<Utc>, then_restart: bool) -> Result<Vec<Effect>, CommandError> {
        match self.state {
            ServerState::Starting { pid, .. } | ServerState::Running { pid, .. } => {
                self.stop_requested = true;
                self.restart_after_stop = then_restart;
                self.state = ServerState::Stopping { since: now, pid };
                Ok(vec![
                    Effect::SendStopCommand,
                    Effect::StartStopTimer(self.policy.stop_timeout),
                ])
            }
            ServerState::Stopping { .. } => {
                self.restart_after_stop |= then_restart;
                Ok(vec![])
            }
            // Stopping a crashed server cancels its pending automatic restart.
            ServerState::Crashed {
                restart_at: Some(_),
                ref exit,
                ..
            } => {
                self.state = ServerState::Stopped {
                    since: Some(now),
                    exit: Some(exit.clone()),
                };
                Ok(vec![Effect::CancelRestart])
            }
            ServerState::Stopped { .. } | ServerState::Crashed { .. } => Err(CommandError::NotRunning),
        }
    }

    fn restart(&mut self, now: DateTime<Utc>) -> Result<Vec<Effect>, CommandError> {
        match self.state {
            ServerState::Stopped { .. } | ServerState::Crashed { .. } => self.start(now),
            _ => self.stop(now, true),
        }
    }

    fn kill(&mut self, now: DateTime<Utc>) -> Result<Vec<Effect>, CommandError> {
        if !self.state.is_active() {
            return Err(CommandError::NotRunning);
        }
        self.stop_requested = true;
        self.restart_after_stop = false;
        self.state = ServerState::Stopping {
            since: now,
            pid: self.state.pid(),
        };
        Ok(vec![Effect::Kill])
    }

    fn spawned(&mut self, pid: Option<u32>) -> Vec<Effect> {
        if let ServerState::Starting { since, .. } = self.state {
            self.state = ServerState::Starting { since, pid };
        }
        vec![]
    }

    fn spawn_failed(&mut self, now: DateTime<Utc>) -> Vec<Effect> {
        if matches!(self.state, ServerState::Starting { .. }) {
            self.state = ServerState::Stopped {
                since: Some(now),
                exit: None,
            };
        }
        vec![]
    }

    fn ready(&mut self, now: DateTime<Utc>) -> Vec<Effect> {
        if let ServerState::Starting { pid, .. } = self.state {
            self.state = ServerState::Running { since: now, pid };
        }
        vec![]
    }

    fn exited(&mut self, exit: ExitInfo, now: DateTime<Utc>) -> Vec<Effect> {
        if !self.state.is_active() {
            return vec![];
        }
        let expected = std::mem::take(&mut self.stop_requested);
        if expected && std::mem::take(&mut self.restart_after_stop) {
            self.begin_start(now);
            return vec![Effect::Spawn];
        }
        // A clean exit nobody asked for while running: the server stopped itself (e.g. a plugin
        // ran `stop`). Before it was ready, the start failed: Minecraft exits with 0 after
        // "Failed to initialize server", and that must not look like a normal stop.
        let was_running = matches!(self.state, ServerState::Running { .. });
        if expected || (exit.success() && was_running) {
            self.state = ServerState::Stopped {
                since: Some(now),
                exit: Some(exit),
            };
            return vec![];
        }
        self.crashed(exit, now)
    }

    fn crashed(&mut self, exit: ExitInfo, now: DateTime<Utc>) -> Vec<Effect> {
        let window = chrono::TimeDelta::from_std(self.policy.window).unwrap_or(chrono::TimeDelta::MAX);
        self.crashes.push_back(now);
        while self.crashes.front().is_some_and(|at| now - *at > window) {
            self.crashes.pop_front();
        }

        let crashes = self.crashes.len();
        let within_limit = u32::try_from(crashes).is_ok_and(|n| n <= self.policy.crash_limit);
        if self.policy.auto_restart && within_limit {
            let delay = BACKOFF[(crashes - 1).min(BACKOFF.len() - 1)];
            let restart_at = now + chrono::TimeDelta::from_std(delay).unwrap_or_default();
            self.state = ServerState::Crashed {
                at: now,
                exit,
                restart_at: Some(restart_at),
            };
            return vec![Effect::ScheduleRestart(delay)];
        }

        self.state = ServerState::Crashed {
            at: now,
            exit,
            restart_at: None,
        };
        if self.policy.auto_restart {
            vec![Effect::CrashLoop { crashes }]
        } else {
            vec![]
        }
    }

    fn stop_timed_out(&mut self) -> Vec<Effect> {
        if matches!(self.state, ServerState::Stopping { .. }) {
            vec![Effect::Kill]
        } else {
            vec![]
        }
    }

    fn restart_due(&mut self, now: DateTime<Utc>) -> Vec<Effect> {
        if matches!(
            self.state,
            ServerState::Crashed {
                restart_at: Some(_),
                ..
            }
        ) {
            self.begin_start(now);
            vec![Effect::Spawn]
        } else {
            vec![]
        }
    }

    fn begin_start(&mut self, now: DateTime<Utc>) {
        self.stop_requested = false;
        self.restart_after_stop = false;
        self.state = ServerState::Starting {
            since: now,
            pid: None,
        };
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn policy() -> RestartPolicy {
        RestartPolicy {
            auto_restart: true,
            crash_limit: 3,
            window: Duration::from_secs(600),
            stop_timeout: Duration::from_secs(60),
        }
    }

    fn crash() -> Input {
        Input::Exited(ExitInfo {
            code: Some(1),
            signal: None,
        })
    }

    fn clean_exit() -> Input {
        Input::Exited(ExitInfo {
            code: Some(0),
            signal: None,
        })
    }

    /// A machine that has started and reported ready.
    fn running(now: DateTime<Utc>) -> Machine {
        let mut machine = Machine::new(policy());
        machine.handle(Input::Start, now).unwrap();
        machine.handle(Input::Spawned { pid: Some(42) }, now).unwrap();
        machine.handle(Input::Ready, now).unwrap();
        machine
    }

    #[test]
    fn normal_lifecycle() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        assert_eq!(machine.handle(Input::Start, now), Ok(vec![Effect::Spawn]));
        assert_eq!(machine.state().name(), "starting");

        machine.handle(Input::Spawned { pid: Some(7) }, now).unwrap();
        machine.handle(Input::Ready, now).unwrap();
        assert!(matches!(
            machine.state(),
            ServerState::Running { pid: Some(7), .. }
        ));

        assert_eq!(
            machine.handle(Input::Stop, now),
            Ok(vec![
                Effect::SendStopCommand,
                Effect::StartStopTimer(Duration::from_secs(60))
            ])
        );
        assert_eq!(machine.state().name(), "stopping");
        machine.handle(clean_exit(), now).unwrap();
        assert!(matches!(
            machine.state(),
            ServerState::Stopped { exit: Some(_), .. }
        ));
    }

    #[test]
    fn invalid_commands_are_rejected() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        assert_eq!(machine.handle(Input::Stop, now), Err(CommandError::NotRunning));
        assert_eq!(machine.handle(Input::Kill, now), Err(CommandError::NotRunning));
        machine.handle(Input::Start, now).unwrap();
        assert_eq!(
            machine.handle(Input::Start, now),
            Err(CommandError::AlreadyRunning)
        );
    }

    #[test]
    fn stop_timeout_escalates_to_kill() {
        let now = Utc::now();
        let mut machine = running(now);
        machine.handle(Input::Stop, now).unwrap();
        assert_eq!(machine.handle(Input::StopTimedOut, now), Ok(vec![Effect::Kill]));
        // A late timer after the process is gone does nothing.
        machine
            .handle(
                Input::Exited(ExitInfo {
                    code: None,
                    signal: Some(9),
                }),
                now,
            )
            .unwrap();
        assert_eq!(machine.state().name(), "stopped");
        assert_eq!(machine.handle(Input::StopTimedOut, now), Ok(vec![]));
    }

    #[test]
    fn requested_stop_is_never_a_crash() {
        let now = Utc::now();
        let mut machine = running(now);
        machine.handle(Input::Kill, now).unwrap();
        machine
            .handle(
                Input::Exited(ExitInfo {
                    code: None,
                    signal: Some(9),
                }),
                now,
            )
            .unwrap();
        assert_eq!(machine.state().name(), "stopped");
    }

    #[test]
    fn console_stop_command_is_a_clean_stop() {
        let now = Utc::now();
        let mut machine = running(now);
        assert_eq!(machine.handle(clean_exit(), now), Ok(vec![]));
        assert_eq!(machine.state().name(), "stopped");
    }

    /// Regression (real Forge/NeoForge runs): "FAILED TO BIND TO PORT" → "Failed to initialize
    /// server" → exit code 0, before the server was ever ready. That is a failed start.
    #[test]
    fn a_clean_exit_before_ready_is_a_failed_start() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        machine.handle(Input::Start, now).unwrap();
        machine.handle(Input::Spawned { pid: Some(7) }, now).unwrap();
        let effects = machine.handle(clean_exit(), now).unwrap();
        assert_eq!(machine.state().name(), "crashed");
        assert_eq!(effects, [Effect::ScheduleRestart(BACKOFF[0])]);
    }

    #[test]
    fn stop_typed_in_the_console_while_starting_is_still_a_stop() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        machine.handle(Input::Start, now).unwrap();
        machine.handle(Input::Spawned { pid: Some(7) }, now).unwrap();
        assert_eq!(machine.handle(Input::StopTyped, now), Ok(vec![]));
        assert_eq!(machine.handle(clean_exit(), now), Ok(vec![]));
        assert_eq!(machine.state().name(), "stopped");
    }

    #[test]
    fn stop_typed_while_stopped_is_ignored_and_does_not_leak_into_the_next_run() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        assert_eq!(machine.handle(Input::StopTyped, now), Ok(vec![]));
        machine.handle(Input::Start, now).unwrap();
        machine.handle(Input::Spawned { pid: Some(7) }, now).unwrap();
        machine.handle(crash(), now).unwrap();
        assert_eq!(machine.state().name(), "crashed");
    }

    #[test]
    fn restart_stops_then_starts() {
        let now = Utc::now();
        let mut machine = running(now);
        let effects = machine.handle(Input::Restart, now).unwrap();
        assert_eq!(effects[0], Effect::SendStopCommand);
        assert_eq!(machine.handle(clean_exit(), now), Ok(vec![Effect::Spawn]));
        assert_eq!(machine.state().name(), "starting");

        // Restarting a stopped server just starts it.
        let mut stopped = Machine::new(policy());
        assert_eq!(stopped.handle(Input::Restart, now), Ok(vec![Effect::Spawn]));
    }

    #[test]
    fn crashes_restart_with_increasing_backoff() {
        let now = Utc::now();
        let mut machine = running(now);
        assert_eq!(
            machine.handle(crash(), now),
            Ok(vec![Effect::ScheduleRestart(BACKOFF[0])])
        );
        assert!(matches!(
            machine.state(),
            ServerState::Crashed {
                restart_at: Some(_),
                ..
            }
        ));

        assert_eq!(machine.handle(Input::RestartDue, now), Ok(vec![Effect::Spawn]));
        machine.handle(Input::Ready, now).unwrap();
        assert_eq!(
            machine.handle(crash(), now),
            Ok(vec![Effect::ScheduleRestart(BACKOFF[1])])
        );
    }

    #[test]
    fn crash_loop_gives_up() {
        let now = Utc::now();
        let mut machine = running(now);
        for _ in 0..3 {
            machine.handle(crash(), now).unwrap();
            machine.handle(Input::RestartDue, now).unwrap();
        }
        assert_eq!(
            machine.handle(crash(), now),
            Ok(vec![Effect::CrashLoop { crashes: 4 }])
        );
        assert!(matches!(
            machine.state(),
            ServerState::Crashed { restart_at: None, .. }
        ));
        assert_eq!(machine.handle(Input::RestartDue, now), Ok(vec![]));
    }

    #[test]
    fn old_crashes_fall_out_of_the_window() {
        let start = Utc::now();
        let mut machine = running(start);
        for minutes in [0, 1, 2] {
            let at = start + TimeDelta::minutes(minutes);
            machine.handle(crash(), at).unwrap();
            machine.handle(Input::RestartDue, at).unwrap();
        }
        // An hour later the old crashes no longer count.
        let later = start + TimeDelta::hours(1);
        assert_eq!(
            machine.handle(crash(), later),
            Ok(vec![Effect::ScheduleRestart(BACKOFF[0])])
        );
    }

    #[test]
    fn auto_restart_can_be_disabled() {
        let now = Utc::now();
        let mut machine = running(now);
        machine.set_policy(RestartPolicy {
            auto_restart: false,
            ..policy()
        });
        assert_eq!(machine.handle(crash(), now), Ok(vec![]));
        assert!(matches!(
            machine.state(),
            ServerState::Crashed { restart_at: None, .. }
        ));
    }

    #[test]
    fn stopping_a_crashed_server_cancels_the_restart() {
        let now = Utc::now();
        let mut machine = running(now);
        machine.handle(crash(), now).unwrap();
        assert_eq!(machine.handle(Input::Stop, now), Ok(vec![Effect::CancelRestart]));
        assert_eq!(machine.state().name(), "stopped");
        assert_eq!(machine.handle(Input::RestartDue, now), Ok(vec![]));
    }

    #[test]
    fn manual_start_during_backoff_cancels_the_timer() {
        let now = Utc::now();
        let mut machine = running(now);
        machine.handle(crash(), now).unwrap();
        assert_eq!(
            machine.handle(Input::Start, now),
            Ok(vec![Effect::CancelRestart, Effect::Spawn])
        );
    }

    #[test]
    fn spawn_failure_returns_to_stopped() {
        let now = Utc::now();
        let mut machine = Machine::new(policy());
        machine.handle(Input::Start, now).unwrap();
        machine.handle(Input::SpawnFailed, now).unwrap();
        assert_eq!(machine.state().name(), "stopped");
    }

    #[test]
    fn states_serialize_with_a_status_tag() {
        let json = serde_json::to_value(ServerState::initial()).unwrap();
        assert_eq!(json["status"], "stopped");
    }
}
