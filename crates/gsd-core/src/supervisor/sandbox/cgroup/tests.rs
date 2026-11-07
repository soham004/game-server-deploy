#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

const APP_PID: u32 = 4242;

/// A directory shaped like a delegated cgroup v2 group. Plain files stand in for the kernel's;
/// the rules are tested here, the kernel's behaviour in [`kernel`] below.
fn fake_group(procs: &[u32]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let pids: Vec<String> = procs.iter().map(u32::to_string).collect();
    for (file, content) in [
        ("cgroup.type", "domain".to_owned()),
        ("cgroup.controllers", "cpuset cpu io memory pids".to_owned()),
        ("cgroup.subtree_control", String::new()),
        ("cgroup.procs", pids.join("\n")),
    ] {
        fs::write(dir.path().join(file), content).unwrap();
    }
    dir
}

/// The control files the kernel would create in a new instance group.
fn fake_instance_files(base: &Path, name: &str) -> PathBuf {
    let dir = base.join("instances").join(name);
    fs::create_dir_all(&dir).unwrap();
    for file in [
        "cgroup.procs",
        "cgroup.kill",
        "memory.swap.max",
        "memory.oom.group",
    ] {
        fs::write(dir.join(file), "").unwrap();
    }
    fs::write(
        dir.join("memory.events"),
        "low 0\nhigh 0\nmax 3\noom 1\noom_kill 1\n",
    )
    .unwrap();
    dir
}

fn app_only() -> Membership {
    Membership {
        app: APP_PID,
        init: None,
    }
}

fn request(name: &str) -> CgroupRequest {
    CgroupRequest {
        name: name.into(),
        limits: CgroupLimits::for_heap(2048),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn the_ceiling_is_the_heap_plus_room_for_the_jvm() {
    // Half the heap again, but at least 1 GiB: small heaps need proportionally more.
    for (heap, ceiling) in [
        (512, 1536),
        (1024, 2048),
        (2048, 3072),
        (4096, 6144),
        (16384, 24576),
    ] {
        assert_eq!(
            CgroupLimits::for_heap(heap).memory_max_mb(),
            ceiling,
            "heap {heap} MB"
        );
    }
    assert_eq!(CgroupLimits::for_heap(1024).tasks_max, DEFAULT_TASKS_MAX);
}

#[test]
fn finds_the_unified_group_in_proc_self_cgroup() {
    assert_eq!(
        parse_own_cgroup("0::/system.slice/app.service\n"),
        Some("/system.slice/app.service")
    );
    // Hybrid hosts list v1 hierarchies too.
    assert_eq!(
        parse_own_cgroup("12:memory:/x\n1:name=systemd:/y\n0::/z\n"),
        Some("/z")
    );
    assert_eq!(parse_own_cgroup("12:memory:/x\n"), None, "v1 only");
}

#[test]
fn a_container_init_may_share_the_group_only_in_its_own_namespace() {
    let in_container = Membership::new(50, 1, "/");
    assert!(in_container.allows(50) && in_container.allows(1));
    assert_eq!(in_container.pids().collect::<Vec<_>>(), [1, 50]);

    // On a host, PID 1 is systemd: never ours to move.
    let on_host = Membership::new(50, 1, "/init.scope");
    assert!(!on_host.allows(1));
    // Started by a shell or anything else: only the app itself.
    let from_shell = Membership::new(50, 7, "/");
    assert!(!from_shell.allows(7));
}

#[test]
fn setup_moves_the_app_out_and_enables_the_controllers() {
    let base = fake_group(&[APP_PID]);
    let tree = CgroupTree::setup(base.path(), &app_only()).unwrap();

    assert_eq!(read(&base.path().join("app/cgroup.procs")), APP_PID.to_string());
    assert_eq!(read(&base.path().join("cgroup.subtree_control")), CONTROLLERS);
    assert_eq!(
        read(&base.path().join("instances/cgroup.subtree_control")),
        CONTROLLERS
    );
    assert_eq!(tree.instances, base.path().join("instances"));

    // Restarting the app in the same group (systemd restart) works the same.
    CgroupTree::setup(base.path(), &app_only()).unwrap();
}

#[test]
fn setup_never_touches_a_group_shared_with_other_processes() {
    let base = fake_group(&[APP_PID, 77]);
    let error = CgroupTree::setup(base.path(), &app_only()).unwrap_err();
    assert!(error.0.contains("Other processes"), "{error}");
    assert!(!base.path().join("app").exists(), "nothing moved");
    assert_eq!(read(&base.path().join("cgroup.subtree_control")), "");
}

#[test]
fn setup_refuses_the_root_group_and_missing_controllers() {
    let root = fake_group(&[APP_PID]);
    fs::remove_file(root.path().join("cgroup.type")).unwrap();
    let error = CgroupTree::setup(root.path(), &app_only()).unwrap_err();
    assert!(error.0.contains("root control group"), "{error}");

    let limited = fake_group(&[APP_PID]);
    fs::write(limited.path().join("cgroup.controllers"), "cpu io").unwrap();
    let error = CgroupTree::setup(limited.path(), &app_only()).unwrap_err();
    assert!(error.0.contains("memory and pids"), "{error}");
}

#[cfg(unix)]
#[test]
fn setup_explains_a_read_only_group() {
    use std::os::unix::fs::PermissionsExt;
    let base = fake_group(&[APP_PID]);
    let control = base.path().join("cgroup.subtree_control");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o444)).unwrap();
    if OpenOptions::new().write(true).open(&control).is_ok() {
        eprintln!("skipped: running as root, permissions don't apply");
        return;
    }
    let error = CgroupTree::setup(base.path(), &app_only()).unwrap_err();
    assert!(
        error.0.contains("isn't writable") && error.0.contains("Delegate=yes"),
        "{error}"
    );
}

#[test]
fn prepare_writes_the_limits_and_clears_leftovers() {
    let base = fake_group(&[APP_PID]);
    let tree = CgroupTree::setup(base.path(), &app_only()).unwrap();
    let dir = fake_instance_files(base.path(), "server-a");
    fs::write(dir.join("cgroup.procs"), "999\n").unwrap(); // left by an earlier run

    let slot = tree.prepare(&request("server-a")).unwrap();
    assert_eq!(read(&dir.join("memory.max")), (3072 * MIB).to_string());
    assert_eq!(read(&dir.join("memory.swap.max")), "0");
    assert_eq!(
        read(&dir.join("memory.oom.group")),
        "1",
        "out of memory stops the whole server"
    );
    assert_eq!(read(&dir.join("pids.max")), DEFAULT_TASKS_MAX.to_string());
    assert_eq!(
        read(&dir.join("cgroup.kill")),
        "1",
        "leftover processes are killed"
    );
    assert_eq!(slot.limits(), CgroupLimits::for_heap(2048));
    assert_eq!(slot.dir(), dir);
}

#[test]
fn only_new_out_of_memory_kills_count() {
    let base = fake_group(&[APP_PID]);
    let tree = CgroupTree::setup(base.path(), &app_only()).unwrap();
    let dir = fake_instance_files(base.path(), "server-a"); // oom_kill 1 from an earlier run

    let slot = tree.prepare(&request("server-a")).unwrap();
    assert!(!slot.memory_limit_hit());
    fs::write(dir.join("memory.events"), "oom 2\noom_kill 2\n").unwrap();
    assert!(slot.memory_limit_hit());
}

#[test]
fn group_names_are_single_path_components() {
    let base = fake_group(&[APP_PID]);
    let tree = CgroupTree::setup(base.path(), &app_only()).unwrap();
    for name in ["", ".", "..", "../escape", "a/b"] {
        let error = tree.prepare(&request(name)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name:?}");
    }
    assert!(!base.path().join("escape").exists());
}

#[test]
fn remove_deletes_an_empty_group() {
    let base = fake_group(&[APP_PID]);
    let tree = CgroupTree::setup(base.path(), &app_only()).unwrap();
    fs::create_dir(base.path().join("instances/gone")).unwrap();
    tree.remove("gone");
    assert!(!base.path().join("instances/gone").exists());
    tree.remove(".."); // ignored
    assert!(base.path().join("instances").exists());
}

/// Against the real kernel: only as root on a cgroup v2 host (WSL as root, CI with sudo, a
/// privileged container). Each test builds a private tree right under the mount.
#[cfg(target_os = "linux")]
pub(crate) mod kernel {
    use std::{
        process::{Child, Stdio},
        sync::atomic::{AtomicU32, Ordering},
        time::Duration,
    };

    use super::*;
    use crate::{
        instances::SandboxIds,
        supervisor::{
            HostSandbox, SandboxSupport,
            process::{self, ProcessSpec},
            sandbox::{ResourceLimits, SandboxSpec, system_read_only_paths, system_read_write_paths},
        },
    };

    /// A real group for one test, with a stand-in "app" process; removed on drop.
    pub(crate) struct RealTree {
        base: PathBuf,
        stand_in: Child,
        host: HostSandbox,
    }

    impl RealTree {
        pub(crate) fn new() -> Option<Self> {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            if !nix::unistd::geteuid().is_root() {
                return skip("needs root");
            }
            let mount = Path::new(CGROUP_MOUNT);
            let name = format!(
                "app-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let base = mount.join(name);
            if let Err(error) = fs::create_dir(&base) {
                return skip(&format!("can't create a control group ({error})"));
            }
            // `setup` moves "the app" into app/; a sleeping child plays it, not the test runner.
            let stand_in = std::process::Command::new("sleep")
                .arg("600")
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let membership = Membership {
                app: stand_in.id(),
                init: None,
            };
            let tree = CgroupTree::setup(&base, &membership).expect("set up a real tree");
            let host = HostSandbox::new(SandboxSupport::detect(), None).with_cgroups(Ok(tree));
            Some(Self { base, stand_in, host })
        }

        fn tree(&self) -> &CgroupTree {
            self.host.cgroups().unwrap()
        }

        /// A host whose servers get this tree's control groups.
        pub(crate) fn host(&self) -> &HostSandbox {
            &self.host
        }
    }

    /// Skips the test (prints why), or fails it where the environment promised these tests can
    /// run (CI sets `REQUIRE_KERNEL_TESTS`), so they can't pass there by silently skipping.
    fn skip(reason: &str) -> Option<RealTree> {
        assert!(
            std::env::var_os("REQUIRE_KERNEL_TESTS").is_none(),
            "kernel test can't run: {reason}"
        );
        eprintln!("skipped: {reason}");
        None
    }

    impl Drop for RealTree {
        fn drop(&mut self) {
            let _ = self.stand_in.kill();
            let _ = self.stand_in.wait();
            // Children first; a group can only be removed once its processes are gone.
            for _ in 0..50 {
                let groups = fs::read_dir(self.base.join("instances"))
                    .into_iter()
                    .flatten()
                    .flatten();
                for group in groups.map(|entry| entry.path()).filter(|path| path.is_dir()) {
                    let _ = fs::write(group.join("cgroup.kill"), "1");
                    let _ = fs::remove_dir(group);
                }
                for dir in ["instances", "app", ""] {
                    let _ = fs::remove_dir(self.base.join(dir));
                }
                if !self.base.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            eprintln!("could not remove {}", self.base.display());
        }
    }

    /// A shell script in the full sandbox, as its own user (the order that matters: the child
    /// joins its group after dropping root).
    fn sandboxed_shell(script: &str, limits: CgroupLimits) -> ProcessSpec {
        let mut read_write = system_read_write_paths();
        read_write.push(std::env::temp_dir());
        ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: "/".into(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            sandbox: Some(SandboxSpec {
                user: Some(SandboxIds {
                    uid: 30998,
                    gid: 30998,
                }),
                read_write,
                read_only: system_read_only_paths(),
                bind_tcp: vec![],
                connect_tcp: vec![],
                limits: ResourceLimits::default(),
                cgroup: Some(CgroupRequest {
                    name: "server".into(),
                    limits,
                }),
            }),
        }
    }

    #[tokio::test]
    async fn a_sandboxed_server_runs_in_its_own_group_with_its_limits() {
        let Some(real) = RealTree::new() else { return };
        let limits = CgroupLimits {
            memory_max: 512 * MIB,
            tasks_max: 100,
        };
        let spec = sandboxed_shell("id -u; cat /proc/self/cgroup", limits);
        let mut spawned = process::spawn(&spec, &real.host).unwrap();
        drop(spawned.stdin);
        let mut stdout = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut spawned.stdout, &mut stdout)
            .await
            .unwrap();
        assert!(spawned.child.wait().await.unwrap().success(), "{stdout}");

        let group = real
            .base
            .strip_prefix(CGROUP_MOUNT)
            .unwrap()
            .join("instances/server");
        assert!(
            stdout.contains(&format!("0::/{}", group.display())),
            "expected {}: {stdout}",
            group.display()
        );
        if real.host.support().separate_users {
            assert!(stdout.starts_with("30998"), "as its own user: {stdout}");
        }
        let dir = real.base.join("instances/server");
        assert_eq!(read(&dir.join("memory.max")).trim(), (512 * MIB).to_string());
        assert_eq!(read(&dir.join("pids.max")).trim(), "100");
        assert!(!spawned.cgroup.unwrap().memory_limit_hit());
    }

    #[tokio::test]
    async fn the_memory_ceiling_stops_a_runaway_process_and_says_so() {
        let Some(real) = RealTree::new() else { return };
        let limits = CgroupLimits {
            memory_max: 48 * MIB,
            tasks_max: 100,
        };
        // `tail -c` keeps the last 300 MB in memory.
        let spec = sandboxed_shell(
            "head -c 400000000 /dev/zero | tail -c 300000000 > /dev/null",
            limits,
        );
        let mut spawned = process::spawn(&spec, &real.host).unwrap();
        let status = tokio::time::timeout(Duration::from_secs(60), spawned.child.wait())
            .await
            .expect("killed long before this")
            .unwrap();
        assert!(!status.success());
        assert!(
            spawned.cgroup.unwrap().memory_limit_hit(),
            "the kernel's OOM kill is detected"
        );
    }

    #[tokio::test]
    async fn the_task_ceiling_stops_a_fork_bomb() {
        let Some(real) = RealTree::new() else { return };
        let limits = CgroupLimits {
            memory_max: 256 * MIB,
            tasks_max: 8,
        };
        let spec = sandboxed_shell(
            "i=0; while [ $i -lt 30 ]; do sleep 1 & i=$((i+1)); done; wait",
            limits,
        );
        let mut spawned = process::spawn(&spec, &real.host).unwrap();
        tokio::time::timeout(Duration::from_secs(30), spawned.child.wait())
            .await
            .unwrap()
            .unwrap();
        let events = read(&real.base.join("instances/server/pids.events"));
        let refused: u64 = events
            .lines()
            .find_map(|line| line.strip_prefix("max "))
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        assert!(refused > 0, "forks beyond the ceiling were refused: {events}");
        assert!(!spawned.cgroup.unwrap().memory_limit_hit());
    }

    #[tokio::test]
    async fn preparing_a_group_again_kills_what_an_earlier_run_left_behind() {
        let Some(real) = RealTree::new() else { return };
        let limits = CgroupLimits::for_heap(512);
        let spec = sandboxed_shell("sleep 600", limits);
        let mut leftover = process::spawn(&spec, &real.host).unwrap();

        real.tree()
            .prepare(spec.sandbox.as_ref().unwrap().cgroup.as_ref().unwrap())
            .unwrap();
        let status = tokio::time::timeout(Duration::from_secs(10), leftover.child.wait())
            .await
            .expect("the leftover was killed")
            .unwrap();
        assert!(!status.success());
    }
}
