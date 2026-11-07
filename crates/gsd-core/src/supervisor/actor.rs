//! The per-instance supervisor actor.
//!
//! One task owns the server process: its stdin, its lifecycle state machine, the recent console
//! lines and the online player list. Everyone else talks to it through a [`SupervisorHandle`]
//! (commands over a channel) and observes it through the event bus. No shared mutable state.
//!
//! Output ordering: reader tasks forward stdout/stderr lines; the waiter task reports the exit
//! only after both readers reached EOF, so the last lines always arrive before "exited".

use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::{mpsc, oneshot, watch},
    time::Instant,
};

use super::{
    process::{self, ProcessSpec},
    sandbox::SandboxSupport,
    state::{CommandError, Effect, ExitInfo, Input, Machine, RestartPolicy, ServerState},
};
use crate::{
    events::{EventBus, EventKind},
    game::{Diagnostic, GameAdapter, GameSignal, LineSource, LogLevel, LogLine, Severity},
    ids::InstanceId,
};

/// Console lines kept in memory per instance.
pub const CONSOLE_HISTORY: usize = 2000;
/// Longest console line kept (longer ones are truncated).
const MAX_LINE_BYTES: usize = 16 * 1024;
/// How long to wait for output pipes to close after the process exited.
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SupervisorError {
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error("the server is not accepting commands right now")]
    NotAcceptingInput,
    #[error("could not write to the server console: {0}")]
    Write(String),
    #[error("the supervisor has shut down")]
    Gone,
}

/// Point-in-time view of an instance's runtime.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub state: ServerState,
    pub players: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub console: Vec<LogLine>,
}

type Reply<T> = oneshot::Sender<Result<T, SupervisorError>>;

pub(crate) enum Command {
    Start {
        spec: ProcessSpec,
        policy: RestartPolicy,
        reply: Reply<()>,
    },
    Restart {
        spec: ProcessSpec,
        policy: RestartPolicy,
        reply: Reply<()>,
    },
    Stop {
        reply: Reply<()>,
    },
    Kill {
        reply: Reply<()>,
    },
    SendLine {
        line: String,
        author: Option<String>,
        reply: Reply<()>,
    },
    Snapshot {
        reply: oneshot::Sender<Snapshot>,
    },
}

enum ProcessEvent {
    Line {
        generation: u64,
        source: LineSource,
        raw: String,
    },
    Exited {
        generation: u64,
        exit: ExitInfo,
    },
}

/// Cheap, cloneable access to one supervisor.
#[derive(Debug, Clone)]
pub struct SupervisorHandle {
    commands: mpsc::Sender<Command>,
    state: watch::Receiver<ServerState>,
}

impl SupervisorHandle {
    pub(crate) fn spawn(
        instance_id: InstanceId,
        adapter: Arc<dyn GameAdapter>,
        events: EventBus,
        support: SandboxSupport,
        policy: RestartPolicy,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(64);
        let (state_tx, state) = watch::channel(ServerState::initial());
        let (process_tx, process_rx) = mpsc::unbounded_channel();
        let actor = Actor {
            instance_id,
            adapter,
            events,
            support,
            machine: Machine::new(policy),
            state_tx,
            spec: None,
            generation: 0,
            stdin: None,
            kill_tx: None,
            process_tx,
            console: VecDeque::with_capacity(CONSOLE_HISTORY),
            seq: 0,
            players: BTreeSet::new(),
            diagnostics: Vec::new(),
            stop_deadline: None,
            restart_at: None,
        };
        tokio::spawn(actor.run(receiver, process_rx));
        Self { commands, state }
    }

    pub async fn start(&self, spec: ProcessSpec, policy: RestartPolicy) -> Result<(), SupervisorError> {
        self.request(|reply| Command::Start { spec, policy, reply }).await
    }

    pub async fn restart(&self, spec: ProcessSpec, policy: RestartPolicy) -> Result<(), SupervisorError> {
        self.request(|reply| Command::Restart { spec, policy, reply })
            .await
    }

    pub async fn stop(&self) -> Result<(), SupervisorError> {
        self.request(|reply| Command::Stop { reply }).await
    }

    pub async fn kill(&self) -> Result<(), SupervisorError> {
        self.request(|reply| Command::Kill { reply }).await
    }

    /// Sends a console command (without a leading `/`). `author` is echoed for the audit trail.
    pub async fn send_line(
        &self,
        line: impl Into<String>,
        author: Option<String>,
    ) -> Result<(), SupervisorError> {
        let line = line.into();
        self.request(|reply| Command::SendLine { line, author, reply })
            .await
    }

    pub async fn snapshot(&self) -> Result<Snapshot, SupervisorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Snapshot { reply })
            .await
            .map_err(|_| SupervisorError::Gone)?;
        response.await.map_err(|_| SupervisorError::Gone)
    }

    /// The current state (updated by the actor on every change).
    pub fn state(&self) -> ServerState {
        self.state.borrow().clone()
    }

    /// Waits until `predicate(state)` holds, or the timeout passes. Returns whether it held.
    pub async fn wait_for(&self, timeout: Duration, predicate: impl Fn(&ServerState) -> bool) -> bool {
        let mut state = self.state.clone();
        tokio::time::timeout(timeout, state.wait_for(|s| predicate(s)))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    async fn request(&self, make: impl FnOnce(Reply<()>) -> Command) -> Result<(), SupervisorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(make(reply))
            .await
            .map_err(|_| SupervisorError::Gone)?;
        response.await.map_err(|_| SupervisorError::Gone)?
    }
}

struct Actor {
    instance_id: InstanceId,
    adapter: Arc<dyn GameAdapter>,
    events: EventBus,
    support: SandboxSupport,
    machine: Machine,
    state_tx: watch::Sender<ServerState>,
    spec: Option<ProcessSpec>,
    /// Incremented per spawned process; events from older processes are ignored.
    generation: u64,
    stdin: Option<ChildStdin>,
    kill_tx: Option<oneshot::Sender<()>>,
    process_tx: mpsc::UnboundedSender<ProcessEvent>,
    console: VecDeque<LogLine>,
    seq: u64,
    players: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    stop_deadline: Option<Instant>,
    restart_at: Option<Instant>,
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut process_rx: mpsc::UnboundedReceiver<ProcessEvent>,
    ) {
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.on_command(command).await,
                    // Every handle is gone: the instance was removed or the app is exiting.
                    None => break,
                },
                Some(event) = process_rx.recv() => self.on_process_event(event).await,
                () = sleep_until(self.stop_deadline) => {
                    self.stop_deadline = None;
                    let _ = self.input(Input::StopTimedOut).await;
                }
                () = sleep_until(self.restart_at) => {
                    self.restart_at = None;
                    let _ = self.input(Input::RestartDue).await;
                }
            }
        }
        if let Some(kill) = self.kill_tx.take() {
            let _ = kill.send(());
        }
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::Start { spec, policy, reply } => {
                self.spec = Some(spec);
                self.machine.set_policy(policy);
                let _ = reply.send(self.input(Input::Start).await);
            }
            Command::Restart { spec, policy, reply } => {
                self.spec = Some(spec);
                self.machine.set_policy(policy);
                let _ = reply.send(self.input(Input::Restart).await);
            }
            Command::Stop { reply } => {
                let _ = reply.send(self.input(Input::Stop).await);
            }
            Command::Kill { reply } => {
                let _ = reply.send(self.input(Input::Kill).await);
            }
            Command::SendLine { line, author, reply } => {
                let _ = reply.send(self.send_line(&line, author.as_deref()).await);
            }
            Command::Snapshot { reply } => {
                let _ = reply.send(Snapshot {
                    state: self.machine.state().clone(),
                    players: self.players.iter().cloned().collect(),
                    diagnostics: self.diagnostics.clone(),
                    console: self.console.iter().cloned().collect(),
                });
            }
        }
    }

    /// Feeds an input (and any follow-ups from effects) through the state machine.
    async fn input(&mut self, input: Input) -> Result<(), SupervisorError> {
        let before = self.machine.state().clone();
        let mut pending = VecDeque::from([input]);
        let mut first = true;
        while let Some(input) = pending.pop_front() {
            let effects = match self.machine.handle(input, Utc::now()) {
                Ok(effects) => effects,
                Err(error) if first => return Err(error.into()),
                Err(_) => continue,
            };
            first = false;
            for effect in effects {
                if let Some(follow_up) = self.perform(effect).await {
                    pending.push_back(follow_up);
                }
            }
        }
        self.publish_state_if_changed(&before);
        Ok(())
    }

    async fn perform(&mut self, effect: Effect) -> Option<Input> {
        match effect {
            Effect::Spawn => Some(self.spawn_process()),
            Effect::SendStopCommand => {
                let stop = self.adapter.stop_command().to_owned();
                if self.write_stdin(&stop).await.is_err() {
                    // Can't ask nicely: escalate right away.
                    return Some(Input::StopTimedOut);
                }
                None
            }
            Effect::StartStopTimer(timeout) => {
                self.stop_deadline = Some(Instant::now() + timeout);
                None
            }
            Effect::Kill => {
                if let Some(kill) = self.kill_tx.take() {
                    let _ = kill.send(());
                }
                None
            }
            Effect::ScheduleRestart(delay) => {
                self.restart_at = Some(Instant::now() + delay);
                self.system_line(
                    LogLevel::Warn,
                    format!("The server crashed. Restarting in {}s.", delay.as_secs()),
                );
                None
            }
            Effect::CancelRestart => {
                self.restart_at = None;
                None
            }
            Effect::CrashLoop { crashes } => {
                let message = format!(
                    "The server crashed {crashes} times in a short period, so automatic restarts are paused."
                );
                self.system_line(LogLevel::Error, message.clone());
                self.add_diagnostic(Diagnostic {
                    code: "crash_loop",
                    severity: Severity::Error,
                    title: "Server keeps crashing".into(),
                    detail: format!("{message} Check the console for the cause, then start it again."),
                });
                None
            }
        }
    }

    fn spawn_process(&mut self) -> Input {
        let Some(spec) = self.spec.clone() else {
            self.system_line(LogLevel::Error, "No launch configuration is available.".into());
            return Input::SpawnFailed;
        };
        let spawned = match process::spawn(&spec, &self.support) {
            Ok(spawned) => spawned,
            Err(error) => {
                self.system_line(
                    LogLevel::Error,
                    format!("Could not start {}: {error}", spec.program.display()),
                );
                return Input::SpawnFailed;
            }
        };

        self.generation += 1;
        self.players.clear();
        self.diagnostics.clear();
        self.stdin = Some(spawned.stdin);
        let (kill_tx, kill_rx) = oneshot::channel();
        self.kill_tx = Some(kill_tx);

        let generation = self.generation;
        let stdout = tokio::spawn(forward_lines(
            spawned.stdout,
            LineSource::Stdout,
            generation,
            self.process_tx.clone(),
        ));
        let stderr = tokio::spawn(forward_lines(
            spawned.stderr,
            LineSource::Stderr,
            generation,
            self.process_tx.clone(),
        ));
        tokio::spawn(wait_for_exit(
            spawned.child,
            kill_rx,
            [stdout, stderr],
            generation,
            self.process_tx.clone(),
        ));

        self.system_line(
            LogLevel::Info,
            format!(
                "Starting server (pid {}).",
                spawned.pid.map_or("?".into(), |p| p.to_string())
            ),
        );
        Input::Spawned { pid: spawned.pid }
    }

    async fn on_process_event(&mut self, event: ProcessEvent) {
        match event {
            ProcessEvent::Line {
                generation,
                source,
                raw,
            } if generation == self.generation => {
                self.on_line(source, raw).await;
            }
            ProcessEvent::Exited { generation, exit } if generation == self.generation => {
                self.stdin = None;
                self.kill_tx = None;
                self.stop_deadline = None;
                self.players.clear();
                let how = match (exit.code, exit.signal) {
                    (Some(code), _) => format!("exit code {code}"),
                    (None, Some(signal)) => format!("signal {signal}"),
                    (None, None) => "unknown status".into(),
                };
                let level = if exit.success() {
                    LogLevel::Info
                } else {
                    LogLevel::Warn
                };
                self.system_line(level, format!("Server process exited ({how})."));
                let _ = self.input(Input::Exited(exit)).await;
            }
            _ => {} // from an earlier process
        }
    }

    async fn on_line(&mut self, source: LineSource, raw: String) {
        // Server output is too noisy for the default log; `<PREFIX>_LOG=…,gsd_core::supervisor=debug` shows it.
        tracing::debug!(instance = %self.instance_id, ?source, "{raw}");
        let parsed = self.adapter.parse_line(&raw);
        let line = self.record_line(LogLine {
            seq: 0,
            received_at: Utc::now(),
            source,
            level: parsed.level,
            thread: parsed.thread,
            text: parsed.text,
            raw,
        });

        if let Some(signal) = self.adapter.detect_signal(&line) {
            match &signal {
                GameSignal::Ready => {
                    let _ = self.input(Input::Ready).await;
                }
                GameSignal::PlayerJoined { name } => {
                    self.players.insert(name.clone());
                }
                GameSignal::PlayerLeft { name } => {
                    self.players.remove(name);
                }
                GameSignal::Saved => {}
            }
            self.events
                .publish(Some(self.instance_id), EventKind::Signal(signal));
        }
        if let Some(diagnostic) = self.adapter.diagnose(&line) {
            self.add_diagnostic(diagnostic);
        }
    }

    async fn send_line(&mut self, line: &str, author: Option<&str>) -> Result<(), SupervisorError> {
        if !matches!(
            self.machine.state(),
            ServerState::Starting { .. } | ServerState::Running { .. }
        ) {
            return Err(SupervisorError::NotAcceptingInput);
        }
        let command = line.trim().trim_start_matches('/');
        if command.is_empty() || command.contains('\n') {
            return Err(SupervisorError::Write(
                "commands must be a single non-empty line".into(),
            ));
        }
        self.write_stdin(command).await?;
        let echo = match author {
            Some(author) => format!("[{author}] > {command}"),
            None => format!("> {command}"),
        };
        self.record_line(LogLine {
            seq: 0,
            received_at: Utc::now(),
            source: LineSource::Input,
            level: LogLevel::Info,
            thread: None,
            text: echo.clone(),
            raw: echo,
        });
        Ok(())
    }

    async fn write_stdin(&mut self, line: &str) -> Result<(), SupervisorError> {
        let stdin = self.stdin.as_mut().ok_or(SupervisorError::NotAcceptingInput)?;
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let result = async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await
        }
        .await;
        result.map_err(|error| SupervisorError::Write(error.to_string()))
    }

    /// A lifecycle message: shown in the console and written to the app log.
    fn system_line(&mut self, level: LogLevel, text: String) {
        let instance = self.instance_id;
        match level {
            LogLevel::Error | LogLevel::Fatal => tracing::error!(%instance, "{text}"),
            LogLevel::Warn => tracing::warn!(%instance, "{text}"),
            _ => tracing::info!(%instance, "{text}"),
        }
        self.record_line(LogLine {
            seq: 0,
            received_at: Utc::now(),
            source: LineSource::System,
            level,
            thread: None,
            raw: text.clone(),
            text,
        });
    }

    /// Numbers the line, keeps it in the history and publishes it.
    fn record_line(&mut self, mut line: LogLine) -> LogLine {
        self.seq += 1;
        line.seq = self.seq;
        if self.console.len() == CONSOLE_HISTORY {
            self.console.pop_front();
        }
        self.console.push_back(line.clone());
        self.events
            .publish(Some(self.instance_id), EventKind::Console(line.clone()));
        line
    }

    fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        if self.diagnostics.iter().any(|known| known.code == diagnostic.code) {
            return;
        }
        tracing::warn!(
            instance = %self.instance_id,
            problem = diagnostic.code,
            "{}: {}",
            diagnostic.title,
            diagnostic.detail
        );
        self.diagnostics.push(diagnostic.clone());
        self.events
            .publish(Some(self.instance_id), EventKind::Diagnostic(diagnostic));
    }

    fn publish_state_if_changed(&mut self, before: &ServerState) {
        let now = self.machine.state().clone();
        if &now != before {
            self.state_tx.send_replace(now.clone());
            self.events.publish(Some(self.instance_id), EventKind::State(now));
        }
    }
}

/// Reads lines (lossy UTF-8, CR/LF trimmed, length-capped) and forwards them to the actor.
async fn forward_lines(
    pipe: impl AsyncRead + Unpin,
    source: LineSource,
    generation: u64,
    sender: mpsc::UnboundedSender<ProcessEvent>,
) {
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::with_capacity(256);
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                buffer.truncate(MAX_LINE_BYTES);
                let raw = String::from_utf8_lossy(&buffer)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned();
                if sender
                    .send(ProcessEvent::Line {
                        generation,
                        source,
                        raw,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

/// Waits for the process (killing it on request), lets the readers drain, then reports the exit.
async fn wait_for_exit(
    mut child: tokio::process::Child,
    kill: oneshot::Receiver<()>,
    readers: [tokio::task::JoinHandle<()>; 2],
    generation: u64,
    sender: mpsc::UnboundedSender<ProcessEvent>,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        Ok(()) = kill => {
            process::kill(&mut child);
            child.wait().await
        }
    };
    for reader in readers {
        let _ = tokio::time::timeout(PIPE_DRAIN_TIMEOUT, reader).await;
    }
    let exit = status.map_or(
        ExitInfo {
            code: None,
            signal: None,
        },
        process::exit_info,
    );
    let _ = sender.send(ProcessEvent::Exited { generation, exit });
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
