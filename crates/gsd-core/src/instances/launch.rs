//! Turns an instance definition into a concrete, sandboxed JVM command line.

use std::{path::PathBuf, time::Duration};

use crate::{
    instances::{
        InstanceDefinition, InstanceLayout,
        definition::{Behavior, JvmPreset, LaunchSpec},
    },
    java::JavaRuntime,
    supervisor::{
        ProcessSpec, RestartPolicy, SandboxSpec,
        sandbox::{
            CgroupLimits, CgroupRequest, DEFAULT_CONNECT_PORTS, ResourceLimits, system_read_only_paths,
            system_read_write_paths,
        },
    },
};

/// Aikar's G1 flags (see the `PaperMC` docs), limited to options every JVM from 8 to 25 accepts.
const AIKAR_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+ParallelRefProcEnabled",
    "-XX:MaxGCPauseMillis=200",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+DisableExplicitGC",
    "-XX:+AlwaysPreTouch",
    "-XX:G1NewSizePercent=30",
    "-XX:G1MaxNewSizePercent=40",
    "-XX:G1HeapRegionSize=8M",
    "-XX:G1ReservePercent=20",
    "-XX:G1HeapWastePercent=5",
    "-XX:G1MixedGCCountTarget=4",
    "-XX:InitiatingHeapOccupancyPercent=15",
    "-XX:G1MixedGCLiveThresholdPercent=90",
    "-XX:SurvivorRatio=32",
    "-XX:+PerfDisableSharedMem",
    "-XX:MaxTenuringThreshold=1",
];

/// Builds the process for `definition` using `java`.
pub fn build_process(
    definition: &InstanceDefinition,
    layout: &InstanceLayout,
    java: &JavaRuntime,
) -> ProcessSpec {
    let java_settings = &definition.java;
    let mut args = vec![
        format!("-Xms{}M", java_settings.min_memory_mb),
        format!("-Xmx{}M", java_settings.max_memory_mb),
    ];
    if java_settings.preset == JvmPreset::Aikar {
        args.extend(AIKAR_FLAGS.iter().map(|flag| (*flag).to_owned()));
    }
    args.extend(common_jvm_args(layout));
    args.extend(java_settings.extra_jvm_args.iter().cloned());

    match &definition.launch {
        LaunchSpec::Jar { jar, args: game_args } => {
            args.extend(["-jar".to_owned(), jar.clone()]);
            args.extend(game_args.iter().cloned());
        }
        LaunchSpec::ArgsFiles {
            jvm_args_file,
            args_file,
            args: game_args,
        } => {
            args.extend([format!("@{jvm_args_file}"), format!("@{args_file}")]);
            args.extend(game_args.iter().cloned());
        }
    }

    ProcessSpec {
        program: java.executable.clone(),
        args,
        cwd: layout.server_dir(),
        env: environment(layout, java),
        sandbox: Some(sandbox_for(definition, layout, java)),
    }
}

/// Heap for loader installers (they patch and remap the game jar, which needs more than the
/// JVM's default on small hosts).
const INSTALLER_HEAP: &str = "-Xmx1G";

/// `java -jar <jar> <args>` for a loader installer, in the server directory and in the same
/// sandbox as the server, except that it may not listen on any port. Installers are downloaded
/// code, so they get no more access than the server itself.
pub fn installer_process(
    definition: &InstanceDefinition,
    layout: &InstanceLayout,
    java: &JavaRuntime,
    jar: &str,
    installer_args: &[String],
) -> ProcessSpec {
    let mut args = vec!["-jar".to_owned(), jar.to_owned()];
    args.extend(installer_args.iter().cloned());
    helper_process(definition, layout, java, layout.server_dir(), &args)
}

/// `java <java_args>` for a short run besides the server (a loader installer, the data generator),
/// from `cwd` (one of the instance's writable folders), in the same sandbox as the server except
/// that it may not listen on any port. Its heap is capped at 1 GiB.
pub fn helper_process(
    definition: &InstanceDefinition,
    layout: &InstanceLayout,
    java: &JavaRuntime,
    cwd: PathBuf,
    java_args: &[String],
) -> ProcessSpec {
    let mut args = vec![INSTALLER_HEAP.to_owned(), "-Djava.awt.headless=true".to_owned()];
    args.extend(common_jvm_args(layout));
    args.extend(java_args.iter().cloned());

    let mut sandbox = sandbox_for(definition, layout, java);
    sandbox.bind_tcp.clear();
    // Not the server's control group: preparing it kills whatever runs there. The helper's
    // heap is capped by INSTALLER_HEAP and it only runs briefly.
    sandbox.cgroup = None;
    ProcessSpec {
        program: java.executable.clone(),
        args,
        cwd,
        env: environment(layout, java),
        sandbox: Some(sandbox),
    }
}

/// JVM flags every process of the instance gets: UTF-8 everywhere, and temp/home inside the
/// instance's own (writable) directories.
fn common_jvm_args(layout: &InstanceLayout) -> [String; 6] {
    [
        // No hsperfdata files in /tmp (also keeps the sandbox's filesystem view minimal).
        "-XX:-UsePerfData".to_owned(),
        "-Dfile.encoding=UTF-8".to_owned(),
        "-Dstdout.encoding=UTF-8".to_owned(),
        "-Dstderr.encoding=UTF-8".to_owned(),
        format!("-Djava.io.tmpdir={}", layout.tmp_dir().display()),
        format!("-Duser.home={}", layout.home_dir().display()),
    ]
}

/// The complete environment (nothing is inherited from the app).
fn environment(layout: &InstanceLayout, java: &JavaRuntime) -> Vec<(String, String)> {
    vec![
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), layout.home_dir().display().to_string()),
        ("TMPDIR".to_owned(), layout.tmp_dir().display().to_string()),
        ("JAVA_HOME".to_owned(), java.home.display().to_string()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("TERM".to_owned(), "dumb".to_owned()),
    ]
}

fn sandbox_for(definition: &InstanceDefinition, layout: &InstanceLayout, java: &JavaRuntime) -> SandboxSpec {
    let mut read_write = layout.writable_dirs().to_vec();
    read_write.extend(system_read_write_paths());
    let mut read_only = vec![java.home.clone()];
    read_only.extend(system_read_only_paths());
    SandboxSpec {
        user: Some(definition.sandbox),
        read_write,
        read_only,
        bind_tcp: std::iter::once(definition.ports.game)
            .chain(definition.ports.control)
            .collect(),
        connect_tcp: DEFAULT_CONNECT_PORTS.to_vec(),
        limits: ResourceLimits::default(),
        cgroup: Some(CgroupRequest {
            name: definition.id.to_string(),
            limits: CgroupLimits::for_heap(definition.java.max_memory_mb),
        }),
    }
}

/// The supervisor's restart rules for an instance.
pub fn restart_policy(behavior: &Behavior) -> RestartPolicy {
    RestartPolicy {
        auto_restart: behavior.auto_restart,
        crash_limit: behavior.crash_limit,
        window: Duration::from_secs(behavior.crash_window_secs),
        stop_timeout: Duration::from_secs(behavior.stop_timeout_secs.max(5)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::instances::definition::tests::sample;

    fn java() -> JavaRuntime {
        JavaRuntime {
            id: "temurin-25-x64".into(),
            home: PathBuf::from("/data/runtimes/java/temurin-25-x64"),
            executable: PathBuf::from("/data/runtimes/java/temurin-25-x64/bin/java"),
            major: 25,
            version: "25.0.1".into(),
            vendor: None,
            managed: true,
        }
    }

    #[test]
    fn builds_a_jar_launch_with_memory_and_flags() {
        let definition = sample();
        let layout = InstanceLayout::new(PathBuf::from("/data/instances/survival"));
        let process = build_process(&definition, &layout, &java());

        assert_eq!(process.program, java().executable);
        assert_eq!(process.cwd, PathBuf::from("/data/instances/survival/server"));
        assert_eq!(&process.args[..2], ["-Xms1024M", "-Xmx4096M"]);
        assert!(process.args.contains(&"-XX:+UseG1GC".to_owned()));
        assert_eq!(
            &process.args[process.args.len() - 3..],
            ["-jar", "server.jar", "nogui"]
        );
        let home = layout.home_dir().display().to_string();
        assert!(process.env.iter().any(|(k, v)| k == "HOME" && *v == home));

        // Hard caps follow the heap: 4096 MB heap + half again for the JVM itself.
        let cgroup = process.sandbox.unwrap().cgroup.unwrap();
        assert_eq!(cgroup.name, definition.id.to_string());
        assert_eq!(cgroup.limits.memory_max_mb(), 6144);
    }

    #[test]
    fn args_file_launches_and_no_preset() {
        let mut definition = sample();
        definition.java.preset = JvmPreset::None;
        definition.launch = LaunchSpec::ArgsFiles {
            jvm_args_file: "user_jvm_args.txt".into(),
            args_file: "libraries/x/unix_args.txt".into(),
            args: vec!["nogui".into()],
        };
        let process = build_process(&definition, &InstanceLayout::new("/i/x".into()), &java());
        assert!(!process.args.contains(&"-XX:+UseG1GC".to_owned()));
        assert!(process.args.ends_with(&[
            "@user_jvm_args.txt".into(),
            "@libraries/x/unix_args.txt".into(),
            "nogui".into()
        ]));
    }

    #[test]
    fn sandbox_allows_only_its_own_dirs_and_port() {
        let definition = sample();
        let layout = InstanceLayout::new(PathBuf::from("/data/instances/survival"));
        let sandbox = build_process(&definition, &layout, &java()).sandbox.unwrap();
        assert_eq!(sandbox.user, Some(definition.sandbox));
        assert!(sandbox.read_write.contains(&layout.server_dir()));
        assert!(
            !sandbox.read_write.contains(&layout.root().to_path_buf()),
            "instance.toml stays read-only"
        );
        assert!(sandbox.read_only.contains(&java().home));
        assert_eq!(sandbox.bind_tcp, vec![definition.ports.game]);
        assert!(sandbox.connect_tcp.contains(&443));
    }

    #[test]
    fn installers_run_in_the_server_sandbox_without_listening() {
        let definition = sample();
        let layout = InstanceLayout::new(PathBuf::from("/data/instances/survival"));
        let args = vec!["--installServer".to_owned()];
        let process = installer_process(&definition, &layout, &java(), "forge-installer.jar", &args);

        assert_eq!(process.program, java().executable);
        assert_eq!(process.cwd, layout.server_dir());
        assert!(process.args.ends_with(&[
            "-jar".into(),
            "forge-installer.jar".into(),
            "--installServer".into()
        ]));
        assert!(process.args.contains(&"-Djava.awt.headless=true".to_owned()));
        assert!(
            !process.args.iter().any(|a| a.starts_with("-Xms")),
            "not the server's memory settings"
        );

        let sandbox = process.sandbox.unwrap();
        let server_sandbox = build_process(&definition, &layout, &java()).sandbox.unwrap();
        assert_eq!(sandbox.user, server_sandbox.user);
        assert_eq!(sandbox.read_write, server_sandbox.read_write);
        assert_eq!(sandbox.read_only, server_sandbox.read_only);
        assert_eq!(sandbox.connect_tcp, server_sandbox.connect_tcp, "may download");
        assert!(sandbox.bind_tcp.is_empty(), "may not listen");
        assert!(
            sandbox.cgroup.is_none(),
            "must not share (and reset) the server's control group"
        );
        assert!(process.env.iter().any(|(k, _)| k == "JAVA_HOME"));
    }

    #[test]
    fn restart_policy_mirrors_behaviour() {
        let policy = restart_policy(&Behavior {
            stop_timeout_secs: 0,
            ..Behavior::default()
        });
        assert!(policy.auto_restart);
        assert_eq!(
            policy.stop_timeout,
            Duration::from_secs(5),
            "timeouts have a floor"
        );
    }
}
