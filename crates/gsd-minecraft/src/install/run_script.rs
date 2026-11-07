//! How to start a server a Forge or `NeoForge` installer just set up.
//!
//! Modern installers (Minecraft 1.17+) write a `run.sh` whose launch line is
//! `java @user_jvm_args.txt @libraries/<…>/unix_args.txt "$@"`. The args-file path changes with
//! every version, so it is read from that line rather than guessed. Legacy Forge (≤ 1.16)
//! instead leaves a `forge-<version>.jar` to run with `-jar`.
//!
//! The installer is untrusted, so paths from `run.sh` must stay inside the server directory.

use std::path::{Component, Path};

use gsd_core::instances::definition::LaunchSpec;

use super::InstallError;

const RUN_SCRIPT: &str = "run.sh";
const USER_JVM_ARGS: &str = "user_jvm_args.txt";

/// Finds the launch for the server in `server_dir`.
pub fn detect(server_dir: &Path) -> Result<LaunchSpec, InstallError> {
    let script = server_dir.join(RUN_SCRIPT);
    if let Ok(text) = std::fs::read_to_string(&script) {
        return from_run_script(&text, server_dir);
    }
    legacy_forge_jar(server_dir)
}

/// The files the installer generated only for starting the server by hand.
pub const SCRIPTS: [&str; 2] = [RUN_SCRIPT, "run.bat"];

/// JVM flags installers write into their args files that break servers inside the sandbox.
///
/// `-Djava.net.preferIPv6Addresses=system` (Forge and `NeoForge` 26.x): the server then can't
/// bind its port (`bind(..) failed with error(-22): Invalid argument`), verified with Forge,
/// `NeoForge` and Purpur 26.3 under Landlock (D-028). Without it Java binds the IPv4 wildcard,
/// which is what every other loader does by default. Our own JVM flags come before the args
/// files, so the only way to override it is to remove it.
const SANDBOX_INCOMPATIBLE_FLAGS: [&str; 1] = ["-Djava.net.preferIPv6Addresses=system"];

/// Removes [`SANDBOX_INCOMPATIBLE_FLAGS`] from an args file; returns the flags it removed.
/// Other lines are kept exactly; a line left empty is dropped.
pub fn strip_incompatible_flags(args_file: &Path) -> Result<Vec<String>, InstallError> {
    let io = |source| InstallError::Io {
        path: args_file.to_owned(),
        source,
    };
    let text = std::fs::read_to_string(args_file).map_err(io)?;
    let mut removed = Vec::new();
    let mut kept_lines = Vec::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        if !body
            .split_whitespace()
            .any(|t| SANDBOX_INCOMPATIBLE_FLAGS.contains(&t))
        {
            kept_lines.push(line.to_owned());
            continue;
        }
        let tokens: Vec<&str> = body.split_whitespace().collect();
        removed.extend(
            tokens
                .iter()
                .filter(|t| SANDBOX_INCOMPATIBLE_FLAGS.contains(t))
                .map(|t| (*t).to_owned()),
        );
        let rest: Vec<&str> = tokens
            .into_iter()
            .filter(|t| !SANDBOX_INCOMPATIBLE_FLAGS.contains(t))
            .collect();
        if !rest.is_empty() {
            kept_lines.push(format!("{}{ending}", rest.join(" ")));
        }
    }
    if !removed.is_empty() {
        std::fs::write(args_file, kept_lines.concat()).map_err(io)?;
    }
    Ok(removed)
}

fn from_run_script(text: &str, server_dir: &Path) -> Result<LaunchSpec, InstallError> {
    let files = launch_line_args_files(text).ok_or_else(|| InstallError::LaunchNotFound {
        detail: format!("{RUN_SCRIPT} has no `java @…` launch line"),
    })?;
    for file in &files {
        if !is_inside(file) {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{RUN_SCRIPT} points outside the server folder: {file}"),
            });
        }
        if !server_dir.join(file).is_file() {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{RUN_SCRIPT} refers to {file}, which the installer did not create"),
            });
        }
    }
    let args_file = files.last().cloned().unwrap_or_default();
    let jvm_args_file = files
        .iter()
        .find(|file| file.ends_with(USER_JVM_ARGS))
        .cloned()
        .unwrap_or_else(|| USER_JVM_ARGS.to_owned());
    if jvm_args_file == args_file {
        return Err(InstallError::LaunchNotFound {
            detail: format!("{RUN_SCRIPT} names no arguments file besides {USER_JVM_ARGS}"),
        });
    }
    ensure_exists(&server_dir.join(&jvm_args_file))?;
    Ok(LaunchSpec::ArgsFiles {
        jvm_args_file,
        args_file,
        args: vec!["nogui".into()],
    })
}

/// The `@file` arguments after `java` on the first launch line that has any. Forge writes
/// `java @…`, `NeoForge` writes `exec java @…`.
fn launch_line_args_files(text: &str) -> Option<Vec<String>> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let tokens: Vec<&str> = line
                .split_whitespace()
                .map(|t| t.trim_matches(['"', '\'']))
                .collect();
            let java = tokens.iter().position(|t| *t == "java" || t.ends_with("/java"))?;
            let files: Vec<String> = tokens[java + 1..]
                .iter()
                .filter_map(|token| token.strip_prefix('@'))
                .map(str::to_owned)
                .collect();
            (!files.is_empty()).then_some(files)
        })
}

/// Legacy Forge: exactly one `forge-*.jar` that isn't the installer.
fn legacy_forge_jar(server_dir: &Path) -> Result<LaunchSpec, InstallError> {
    let entries = std::fs::read_dir(server_dir).map_err(|source| InstallError::Io {
        path: server_dir.to_owned(),
        source,
    })?;
    let mut jars: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.starts_with("forge-")
                && Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
                && !name.contains("installer")
                && !name.contains("shim")
        })
        .collect();
    jars.sort();
    match jars.as_slice() {
        [jar] => Ok(LaunchSpec::Jar {
            jar: jar.clone(),
            args: vec!["nogui".into()],
        }),
        [] => Err(InstallError::LaunchNotFound {
            detail: format!("the installer created neither {RUN_SCRIPT} nor a forge-*.jar"),
        }),
        several => Err(InstallError::LaunchNotFound {
            detail: format!("several Forge jars to choose from: {}", several.join(", ")),
        }),
    }
}

/// Relative, without `..`: stays inside the server directory.
fn is_inside(file: &str) -> bool {
    let path = Path::new(file);
    !file.is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Some installers only reference `user_jvm_args.txt`; an empty one keeps the launch valid.
fn ensure_exists(path: &Path) -> Result<(), InstallError> {
    if path.is_file() {
        return Ok(());
    }
    std::fs::write(path, "").map_err(|source| InstallError::Io {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture_text;

    /// A server dir with `run.sh` and the files it refers to.
    fn installed(run_sh: &str, files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(RUN_SCRIPT), run_sh).unwrap();
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "-jar x.jar").unwrap();
        }
        dir
    }

    #[test]
    fn reads_the_real_forge_26_3_run_script() {
        let args = "libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt";
        let dir = installed(&fixture_text("forge_26.3_run.sh"), &[USER_JVM_ARGS, args]);
        assert_eq!(
            detect(dir.path()).unwrap(),
            LaunchSpec::ArgsFiles {
                jvm_args_file: USER_JVM_ARGS.into(),
                args_file: args.into(),
                args: vec!["nogui".into()],
            }
        );
    }

    #[test]
    fn reads_the_real_neoforge_26_3_run_script() {
        let args = "libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt";
        let dir = installed(&fixture_text("neoforge_26.3_run.sh"), &[USER_JVM_ARGS, args]);
        let LaunchSpec::ArgsFiles { args_file, .. } = detect(dir.path()).unwrap() else {
            panic!("expected args files");
        };
        assert_eq!(args_file, args);
    }

    #[test]
    fn accepts_exec_and_a_full_java_path() {
        for line in [
            "exec java @user_jvm_args.txt @libraries/x/unix_args.txt \"$@\"",
            "\"/usr/lib/jvm/bin/java\" @user_jvm_args.txt @libraries/x/unix_args.txt",
        ] {
            let dir = installed(
                &format!("#!/bin/sh\n{line}\n"),
                &[USER_JVM_ARGS, "libraries/x/unix_args.txt"],
            );
            assert!(
                matches!(detect(dir.path()), Ok(LaunchSpec::ArgsFiles { .. })),
                "{line}"
            );
        }
    }

    #[test]
    fn creates_a_missing_user_jvm_args_file() {
        let dir = installed(
            "exec java @libraries/x/unix_args.txt \"$@\"\n",
            &["libraries/x/unix_args.txt"],
        );
        assert!(matches!(
            detect(dir.path()).unwrap(),
            LaunchSpec::ArgsFiles { .. }
        ));
        assert!(dir.path().join(USER_JVM_ARGS).is_file());
    }

    #[test]
    fn rejects_paths_that_escape_the_server_folder() {
        for evil in ["@../../etc/passwd", "@/etc/passwd", "@libraries/../../../x"] {
            let dir = installed(&format!("java @user_jvm_args.txt {evil}\n"), &[USER_JVM_ARGS]);
            assert!(
                matches!(detect(dir.path()), Err(InstallError::LaunchNotFound { .. })),
                "{evil}"
            );
        }
    }

    #[test]
    fn rejects_scripts_without_a_usable_launch_line() {
        let cases = [
            "#!/bin/sh\necho hello\n",
            "# java @user_jvm_args.txt @libraries/x/unix_args.txt\n",
            "java -jar server.jar\n",
            "java @user_jvm_args.txt \"$@\"\n",
            "echo @libraries/x/unix_args.txt java\n",
        ];
        for script in cases {
            let dir = installed(script, &[USER_JVM_ARGS]);
            assert!(detect(dir.path()).is_err(), "{script:?}");
        }
        let missing = installed(
            "java @user_jvm_args.txt @libraries/gone/unix_args.txt\n",
            &[USER_JVM_ARGS],
        );
        let error = detect(missing.path()).unwrap_err().to_string();
        assert!(error.contains("did not create"), "{error}");
    }

    #[test]
    fn strips_the_ipv6_flag_from_real_forge_and_neoforge_args_files() {
        let dir = tempfile::tempdir().unwrap();
        // Forge 26.3: one line. NeoForge 26.3: one argument per line (both as shipped).
        let forge = dir.path().join("forge_unix_args.txt");
        std::fs::write(
            &forge,
            "-Djava.net.preferIPv6Addresses=system -XX:+UseCompactObjectHeaders -XX:StackShadowPages=32 -jar forge-26.3-66.1.0-shim.jar\n",
        )
        .unwrap();
        let neoforge = dir.path().join("neoforge_unix_args.txt");
        std::fs::write(
            &neoforge,
            "--add-opens java.base/java.lang.invoke=ALL-UNNAMED\n-Djava.net.preferIPv6Addresses=system\r\n-DlibraryDirectory=libraries\n-classpath\nlibraries/a.jar:libraries/b.jar\nnet.neoforged.Main",
        )
        .unwrap();

        assert_eq!(
            strip_incompatible_flags(&forge).unwrap(),
            ["-Djava.net.preferIPv6Addresses=system"]
        );
        assert_eq!(
            std::fs::read_to_string(&forge).unwrap(),
            "-XX:+UseCompactObjectHeaders -XX:StackShadowPages=32 -jar forge-26.3-66.1.0-shim.jar\n"
        );
        assert_eq!(strip_incompatible_flags(&neoforge).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(&neoforge).unwrap(),
            "--add-opens java.base/java.lang.invoke=ALL-UNNAMED\n-DlibraryDirectory=libraries\n-classpath\nlibraries/a.jar:libraries/b.jar\nnet.neoforged.Main",
            "every other line is untouched, the last one without a newline too"
        );
    }

    #[test]
    fn files_without_the_flag_are_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("unix_args.txt");
        std::fs::write(&file, "-Djava.net.preferIPv6Addresses=true -jar x.jar\n").unwrap();
        assert_eq!(strip_incompatible_flags(&file).unwrap(), Vec::<String>::new());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "-Djava.net.preferIPv6Addresses=true -jar x.jar\n"
        );
    }

    #[test]
    fn legacy_forge_launches_its_jar() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "forge-1.16.5-36.2.39.jar",
            "forge-installer.jar",
            "minecraft_server.1.16.5.jar",
        ] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        assert_eq!(
            detect(dir.path()).unwrap(),
            LaunchSpec::Jar {
                jar: "forge-1.16.5-36.2.39.jar".into(),
                args: vec!["nogui".into()],
            }
        );

        std::fs::write(dir.path().join("forge-1.16.5-36.2.38.jar"), "").unwrap();
        assert!(detect(dir.path()).unwrap_err().to_string().contains("several"));
        assert!(detect(tempfile::tempdir().unwrap().path()).is_err());
    }
}
