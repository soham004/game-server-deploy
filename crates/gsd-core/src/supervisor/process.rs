//! Starting and killing processes: long-running servers (driven by the supervisor actor) and
//! short-lived ones run to completion (loader installers).

use std::{io, path::PathBuf, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "linux")]
use super::sandbox;
use super::{
    sandbox::{HostSandbox, SandboxSpec, SandboxSupport, cgroup::CgroupSlot},
    state::ExitInfo,
};

/// Everything needed to start one server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The complete environment on Unix (nothing is inherited); added to the inherited one on Windows.
    pub env: Vec<(String, String)>,
    pub sandbox: Option<SandboxSpec>,
}

pub(crate) struct Spawned {
    pub pid: Option<u32>,
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
    /// The control group the process runs in, when the host has hard limits.
    pub cgroup: Option<CgroupSlot>,
}

/// The command that starts `spec`: through the launcher where there is one, else directly.
fn start_command(spec: &ProcessSpec, host: &HostSandbox, cgroup: Option<&CgroupSlot>) -> io::Result<Command> {
    #[cfg(target_os = "linux")]
    if let Some(launcher) = host.launcher() {
        return sandbox::launch_command(launcher, spec, &host.support(), cgroup);
    }
    // Only a launcher can apply a sandbox, so going on here would silently run without one.
    let needs_launcher = spec.sandbox.is_some() && host.support() != SandboxSupport::none();
    if needs_launcher || cgroup.is_some() {
        return Err(io::Error::other(
            "this host's sandbox needs a launcher, and has none",
        ));
    }
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).current_dir(&spec.cwd);
    Ok(command)
}

pub(crate) fn spawn(spec: &ProcessSpec, host: &HostSandbox) -> io::Result<Spawned> {
    // Hard limits only where the process asks for them and the host has control groups.
    let request = spec.sandbox.as_ref().and_then(|sandbox| sandbox.cgroup.as_ref());
    let cgroup = match request.zip(host.cgroups()) {
        Some((request, tree)) => Some(tree.prepare(request)?),
        None => None,
    };

    let mut command = start_command(spec, host, cgroup.as_ref())?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Don't leak the app's environment (API keys, tokens) into game servers. Windows programs
    // need a few system variables, so there we only add ours.
    #[cfg(unix)]
    command.env_clear();
    command.envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));

    // Own process group, so a kill also reaches anything the server started.
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let missing = |name: &str| io::Error::other(format!("child {name} was not captured"));
    let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok(Spawned {
        pid: child.id(),
        child,
        stdin,
        stdout,
        stderr,
        cgroup,
    })
}

/// How a [`run_to_completion`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Exited(ExitInfo),
    /// `cancel` fired; the process group was killed.
    Cancelled,
}

/// Longest output line passed on (longer lines are truncated).
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024;
/// Output lines that may wait to be handled. A process that prints faster than that waits on its
/// pipe, as it would with a slow terminal, instead of filling the app's memory.
pub(crate) const LINE_BACKLOG: usize = 1024;
/// How long to keep reading buffered output after the process exited.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs a short-lived process (a loader installer, a probe) to completion, sandboxed like a
/// server. Every stdout/stderr line goes to `on_line`; stdin is closed. When `cancel` fires the
/// whole process group is killed. Dropping the future kills the process too (`kill_on_drop`),
/// so callers can bound it with `tokio::time::timeout`.
pub async fn run_to_completion(
    spec: &ProcessSpec,
    host: &HostSandbox,
    cancel: &CancellationToken,
    mut on_line: impl FnMut(&str),
) -> io::Result<RunOutcome> {
    let mut spawned = spawn(spec, host)?;
    drop(spawned.stdin);
    let (lines_tx, mut lines) = mpsc::channel(LINE_BACKLOG);
    tokio::spawn(send_lines(spawned.stdout, lines_tx.clone()));
    tokio::spawn(send_lines(spawned.stderr, lines_tx));

    let group = spawned.pid;
    let status = loop {
        tokio::select! {
            Some(line) = lines.recv() => on_line(&line),
            status = spawned.child.wait() => {
                // What it started in its group must not outlive it.
                kill_group(group);
                break status?;
            }
            () = cancel.cancelled() => {
                kill(&mut spawned.child);
                let _ = spawned.child.wait().await;
                return Ok(RunOutcome::Cancelled);
            }
        }
    };
    // Output still in the pipes arrives after the exit; pass it on (bounded, in case a
    // grandchild keeps the pipe open).
    let drain = async {
        while let Some(line) = lines.recv().await {
            on_line(&line);
        }
    };
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, drain).await;
    Ok(RunOutcome::Exited(exit_info(status)))
}

/// Forwards lines (see [`read_line`]) until EOF.
async fn send_lines(pipe: impl AsyncRead + Unpin, sender: mpsc::Sender<String>) {
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::with_capacity(256);
    while let Ok(Some(line)) = read_line(&mut reader, &mut buffer).await {
        if sender.send(line).await.is_err() {
            break;
        }
    }
}

/// The next line of a process's output: lossy UTF-8, without its line ending, and at most
/// [`MAX_LINE_BYTES`] of it. The rest of a longer line is read and dropped as it comes, so output
/// that never ends its line can't grow the app's memory. `None` at the end of the output.
pub(crate) async fn read_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    buffer: &mut Vec<u8>,
) -> io::Result<Option<String>> {
    buffer.clear();
    let mut read_any = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        read_any = true;
        let line_end = available.iter().position(|byte| *byte == b'\n');
        let part = &available[..line_end.unwrap_or(available.len())];
        let room = MAX_LINE_BYTES.saturating_sub(buffer.len());
        buffer.extend_from_slice(&part[..part.len().min(room)]);
        let consumed = part.len() + usize::from(line_end.is_some());
        reader.consume(consumed);
        if line_end.is_some() {
            break;
        }
    }
    if !read_any {
        return Ok(None);
    }
    let line = String::from_utf8_lossy(buffer);
    Ok(Some(line.trim_end_matches('\r').to_owned()))
}

/// Kills the process and its whole process group.
pub(crate) fn kill(child: &mut Child) {
    kill_group(child.id());
    let _ = child.start_kill();
}

/// Kills the process group `pid` leads, if any of it is left. Also used once the leader has exited
/// by itself: what it started in its group must not outlive it.
pub(crate) fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

pub(crate) fn exit_info(status: std::process::ExitStatus) -> ExitInfo {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    ExitInfo {
        code: status.code(),
        signal,
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn shell(script: &str) -> ProcessSpec {
        ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            sandbox: None,
        }
    }

    async fn run(script: &str, cancel: &CancellationToken) -> (RunOutcome, Vec<String>) {
        let mut lines = Vec::new();
        let outcome = run_to_completion(&shell(script), &HostSandbox::disabled(), cancel, |line| {
            lines.push(line.to_owned());
        })
        .await
        .unwrap();
        (outcome, lines)
    }

    #[tokio::test]
    async fn collects_stdout_and_stderr_and_the_exit_code() {
        let (outcome, lines) = run(
            "echo out; echo err >&2; printf 'no newline'; exit 3",
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome,
            RunOutcome::Exited(ExitInfo {
                code: Some(3),
                signal: None
            })
        );
        for expected in ["out", "err", "no newline"] {
            assert!(lines.iter().any(|line| line == expected), "{lines:?}");
        }
    }

    #[tokio::test]
    async fn output_written_just_before_exit_is_not_lost() {
        let (_, lines) = run(
            "i=0; while [ $i -lt 500 ]; do echo line$i; i=$((i+1)); done",
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(lines.len(), 500);
        assert_eq!(lines.last().map(String::as_str), Some("line499"));
    }

    #[tokio::test]
    async fn cancelling_kills_the_process_group() {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            trigger.cancel();
        });
        let started = Instant::now();
        let (outcome, _) = run("sleep 30 & sleep 30", &cancel).await;
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_background_child_holding_the_pipe_does_not_block_completion() {
        let started = Instant::now();
        let (outcome, lines) = run("sleep 4 & echo done", &CancellationToken::new()).await;
        assert!(matches!(outcome, RunOutcome::Exited(exit) if exit.code == Some(0)));
        assert_eq!(lines, ["done"]);
        assert!(started.elapsed() < Duration::from_secs(4), "drain is bounded");
    }

    fn is_alive(pid: &str) -> bool {
        std::path::Path::new("/proc").join(pid.trim()).exists()
    }

    /// What a process started in its group must not go on running after it: its files would keep
    /// changing under whatever the app does next.
    #[tokio::test]
    async fn a_child_left_in_the_group_is_stopped_when_the_process_exits() {
        let (outcome, lines) = run("sleep 300 & echo $!; exit 0", &CancellationToken::new()).await;
        assert!(matches!(outcome, RunOutcome::Exited(exit) if exit.code == Some(0)));
        let child = lines.first().expect("the child's pid was printed");
        for _ in 0..50 {
            if !is_alive(child) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the child {child} outlived the process that started it");
    }

    async fn lines_of(output: &[u8]) -> Vec<String> {
        let mut reader = BufReader::with_capacity(64, output);
        let (mut buffer, mut lines) = (Vec::new(), Vec::new());
        while let Some(line) = read_line(&mut reader, &mut buffer).await.unwrap() {
            lines.push(line);
        }
        lines
    }

    #[tokio::test]
    async fn lines_are_split_trimmed_and_the_last_one_needs_no_ending() {
        assert_eq!(lines_of(b"one\r\ntwo\n\nlast").await, ["one", "two", "", "last"]);
        assert_eq!(lines_of(b"").await, Vec::<String>::new());
        assert_eq!(lines_of(b"\n").await, [""]);
        assert_eq!(lines_of(&[b'a', 0xff, b'b', b'\n']).await, ["a\u{fffd}b"]);
    }

    /// A line is kept up to the limit and the rest of it dropped as it is read, so the line after
    /// it still arrives whole.
    #[tokio::test]
    async fn an_overlong_line_is_cut_and_the_next_one_is_intact() {
        let mut output = vec![b'x'; MAX_LINE_BYTES * 3 + 17];
        output.extend_from_slice(b"\nnext\n");
        let lines = lines_of(&output).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "next");
    }

    /// Output that never ends its line used to be collected until the line ended: never. Half a
    /// gigabyte goes through here; if any of it were kept, this test's memory would show it, and
    /// reading it into a buffer first would take far longer than this allows.
    #[tokio::test]
    async fn output_that_never_ends_its_line_does_not_grow_memory() {
        let started = Instant::now();
        let (outcome, lines) = run(
            "head -c 536870912 /dev/zero | tr '\\0' 'x'; echo; echo after",
            &CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, RunOutcome::Exited(exit) if exit.code == Some(0)));
        assert_eq!(lines.len(), 2, "one cut line, then the next");
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "after");
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    /// Silently running without the sandbox would be the worst way for this to go wrong.
    #[tokio::test]
    async fn a_sandbox_without_a_launcher_refuses_to_start() {
        use crate::supervisor::sandbox::ResourceLimits;

        let mut spec = shell("true");
        spec.sandbox = Some(SandboxSpec {
            user: None,
            read_write: vec![],
            read_only: vec![],
            bind_tcp: vec![],
            connect_tcp: vec![],
            limits: ResourceLimits::default(),
            cgroup: None,
        });
        let mut support = SandboxSupport::none();
        support.filesystem = true;
        let result = spawn(&spec, &HostSandbox::new(support, None));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let mut spec = shell("true");
        spec.program = "/nonexistent/program".into();
        let result =
            run_to_completion(&spec, &HostSandbox::disabled(), &CancellationToken::new(), |_| {}).await;
        assert!(result.is_err());
    }
}
