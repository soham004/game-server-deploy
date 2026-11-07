//! Starting and killing server processes.

use std::{io, path::PathBuf, process::Stdio};

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::{
    sandbox::{self, SandboxSpec, SandboxSupport},
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
}

pub(crate) fn spawn(spec: &ProcessSpec, support: &SandboxSupport) -> io::Result<Spawned> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
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

    if let Some(sandbox_spec) = &spec.sandbox {
        sandbox::apply(&mut command, sandbox_spec, support)?;
    }

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
    })
}

/// Kills the process and its whole process group.
pub(crate) fn kill(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
    let _ = child.start_kill();
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
