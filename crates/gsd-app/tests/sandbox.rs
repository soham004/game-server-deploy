//! The sandbox with the real launcher and probe (the `gsd` binary) and the real kernel.
//! The checks through the data-dir probe skip where Landlock is missing or when running as root
//! (the probe would then run as another user, who can't reach the build directory).

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::Path,
    time::{Duration, Instant},
};

use gsd_core::supervisor::{
    HostSandbox, ProcessSpec, SandboxSpec, SandboxSupport,
    process::{RunOutcome, run_to_completion},
    sandbox::{ResourceLimits, check, system_read_only_paths, system_read_write_paths},
};
use tokio_util::sync::CancellationToken;

const APP: &str = env!("CARGO_BIN_EXE_gsd");

fn usable_support() -> Option<SandboxSupport> {
    let support = SandboxSupport::detect();
    if !support.filesystem || support.separate_users {
        eprintln!("skipping: needs Landlock and a non-root user");
        return None;
    }
    Some(support)
}

fn real_host() -> HostSandbox {
    HostSandbox::new(SandboxSupport::detect(), None).with_launcher(APP.into())
}

/// `script` for `sh`, in a sandbox that may only write `allowed`.
fn sandboxed(allowed: &Path, script: &str) -> ProcessSpec {
    let mut read_write = vec![allowed.to_owned()];
    read_write.extend(system_read_write_paths());
    ProcessSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: allowed.to_owned(),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        sandbox: Some(SandboxSpec {
            user: None,
            read_write,
            read_only: system_read_only_paths(),
            bind_tcp: vec![],
            connect_tcp: vec![],
            limits: ResourceLimits::default(),
            cgroup: None,
        }),
    }
}

/// Runs `script` through the launcher: whether it succeeded, and its output.
async fn run_sandboxed(allowed: &Path, script: &str) -> (bool, String) {
    let mut output = String::new();
    let outcome = run_to_completion(
        &sandboxed(allowed, script),
        &real_host(),
        &CancellationToken::new(),
        |line| {
            output.push_str(line);
            output.push('\n');
        },
    )
    .await
    .unwrap();
    let RunOutcome::Exited(exit) = outcome else {
        panic!("not cancelled");
    };
    (exit.code == Some(0), output)
}

#[tokio::test]
async fn a_server_can_use_its_own_folder_on_a_local_filesystem() {
    let Some(support) = usable_support() else { return };
    let data = tempfile::tempdir().unwrap();

    let usable = check::own_folder_usable(Path::new(APP), &data.path().join("sandbox-check"), support)
        .await
        .unwrap();
    assert!(
        usable,
        "the sandbox should allow a server's own folder on {}",
        data.path().display()
    );
}

#[tokio::test]
async fn the_check_cleans_up_after_itself() {
    let Some(support) = usable_support() else { return };
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().join("sandbox-check");

    check::own_folder_usable(Path::new(APP), &dir, support)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "scratch folders are removed"
    );
}

#[tokio::test]
async fn landlock_confines_the_filesystem() {
    if SandboxSupport::detect().landlock_abi.is_none() {
        eprintln!("skipped: Landlock is not available on this kernel");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let own = root.path().join("own");
    let sibling = root.path().join("sibling");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("secret.txt"), "top secret").unwrap();

    let (ok, _) = run_sandboxed(&own, &format!("echo hi > {}/mine.txt", own.display())).await;
    assert!(ok, "writing its own directory is allowed");
    assert!(own.join("mine.txt").exists());

    let (ok, output) = run_sandboxed(&own, &format!("cat {}/secret.txt", sibling.display())).await;
    assert!(!ok, "reading a sibling must fail, got: {output}");
    assert!(!output.contains("top secret"));

    let (ok, _) = run_sandboxed(&own, &format!("echo x > {}/planted.txt", sibling.display())).await;
    assert!(!ok);
    assert!(!sibling.join("planted.txt").exists());

    let (ok, _) = run_sandboxed(&own, "ls /usr/bin > /dev/null").await;
    assert!(ok, "system directories stay readable");
}

/// Regression: `/etc/resolv.conf` is usually a symlink out of `/etc` (systemd-resolved, WSL);
/// without its target in the rules, every DNS lookup in the sandbox failed.
#[tokio::test]
async fn name_resolution_config_stays_readable() {
    if SandboxSupport::detect().landlock_abi.is_none() {
        eprintln!("skipped: Landlock is not available on this kernel");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for file in ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf"] {
        if Path::new(file).exists() {
            let (ok, output) = run_sandboxed(dir.path(), &format!("cat {file} > /dev/null")).await;
            assert!(ok, "{file} must be readable in the sandbox: {output}");
        }
    }
    let (ok, output) = run_sandboxed(dir.path(), "getent hosts localhost").await;
    assert!(ok, "resolving a name must work: {output}");
}

#[tokio::test]
async fn open_file_limit_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, output) = run_sandboxed(dir.path(), "ulimit -n").await;
    assert!(ok, "{output}");
    let limit: u64 = output.trim().parse().unwrap();
    assert!(limit <= ResourceLimits::default().open_files, "limit {limit}");
}

/// After the launcher, the process is the program itself: same pid, its own arguments, the
/// environment it was given and the folder it asked for.
#[tokio::test]
async fn the_launcher_becomes_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, output) = run_sandboxed(
        dir.path(),
        "echo \"args=$0\"; pwd -P; tr '\\0' '\\n' < /proc/$$/environ | grep -c .; tr '\\0' ' ' < /proc/$$/cmdline; echo",
    )
    .await;
    assert!(ok, "{output}");
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines[0], "args=/bin/sh");
    assert_eq!(
        Path::new(lines[1]),
        dir.path().canonicalize().unwrap(),
        "in its own folder"
    );
    assert_eq!(lines[2], "1", "only the environment it was given: {output}");
    assert!(lines[3].starts_with("/bin/sh -c"), "no launcher left: {output}");
    assert!(!output.contains("sandbox-launch"), "{output}");
}

/// A launcher that can't do its job must not run the program at all, and must say why.
#[tokio::test]
async fn a_launcher_that_fails_runs_nothing_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let mut spec = sandboxed(dir.path(), "true");
    spec.program = "/bin/this-program-is-not-there".into();
    let result = run_to_completion(&spec, &real_host(), &CancellationToken::new(), |_| {}).await;
    assert!(result.is_err(), "a missing program is an error from starting");

    // It exists, so only the launcher finds out it can't run it: the file isn't executable.
    let script = dir.path().join("not-executable");
    std::fs::write(&script, format!("#!/bin/sh\necho ran > {}\n", marker.display())).unwrap();
    spec.program = script;
    let mut output = String::new();
    let outcome = run_to_completion(&spec, &real_host(), &CancellationToken::new(), |line| {
        output.push_str(line);
    })
    .await
    .unwrap();
    assert!(
        matches!(&outcome, RunOutcome::Exited(exit) if exit.code == Some(126)),
        "{outcome:?}"
    );
    assert!(output.contains("Could not start the server"), "{output}");
    assert!(!marker.exists());
}

/// The signal follows the thread that started the process, so a thread stands in for the app.
/// `forget` keeps `kill_on_drop` out of it: only the signal can stop the script.
#[test]
fn a_process_is_told_to_stop_when_the_app_dies() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("stopped");
    let script = format!(
        "trap 'echo saved > {}; exit 0' TERM; echo ready; while :; do sleep 0.1; done",
        marker.display()
    );
    let spec = sandboxed(dir.path(), &script);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let host = real_host();
            let cancel = CancellationToken::new();
            let (ready_tx, ready) = tokio::sync::oneshot::channel();
            let mut ready_tx = Some(ready_tx);
            let mut run = Box::pin(run_to_completion(&spec, &host, &cancel, |line| {
                if line == "ready"
                    && let Some(ready_tx) = ready_tx.take()
                {
                    let _ = ready_tx.send(());
                }
            }));
            // Only once the script has set its trap.
            tokio::select! {
                outcome = &mut run => panic!("the script ended early: {outcome:?}"),
                _ = ready => {}
            }
            std::mem::forget(run);
        });
    })
    .join()
    .unwrap();

    let started = Instant::now();
    while !marker.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the process got no SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
