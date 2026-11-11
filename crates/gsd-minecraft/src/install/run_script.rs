//! How to start a server a Forge or `NeoForge` installer just set up.
//!
//! Modern installers (Minecraft 1.17+) write a `run.sh` whose launch line is
//! `java @user_jvm_args.txt @libraries/<…>/unix_args.txt "$@"`. The args-file path changes with
//! every version, so it is read from that line rather than guessed. Legacy Forge (≤ 1.16)
//! instead leaves a `forge-<version>.jar` to run with `-jar`.
//!
//! The installer is untrusted, so paths from `run.sh` must stay inside the server directory, and
//! every file here is read and written through the file manager, which a link can't lead out of it.

use std::path::{Component, Path};

use gsd_core::{
    files::{EntryKind, FilesError, SafeRelPath, ServerFiles},
    instances::definition::LaunchSpec,
};

use super::InstallError;

const RUN_SCRIPT: &str = "run.sh";
const USER_JVM_ARGS: &str = "user_jvm_args.txt";

/// Finds the launch for the server in the folder `files` works in.
pub async fn detect(files: &ServerFiles) -> Result<LaunchSpec, InstallError> {
    match files.read_text(path(RUN_SCRIPT)?).await {
        Ok(script) => from_run_script(&script.content, files).await,
        // No script, or nothing that reads as one: legacy Forge, if anything.
        Err(_) => legacy_forge_jar(files).await,
    }
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

/// Removes [`SANDBOX_INCOMPATIBLE_FLAGS`] from the args file `args_file`; returns the flags it
/// removed. Other lines are kept exactly; a line left empty is dropped.
pub async fn strip_incompatible_flags(
    files: &ServerFiles,
    args_file: &str,
) -> Result<Vec<String>, InstallError> {
    let file = files.read_text(path(args_file)?).await?;
    let (kept, removed) = without_incompatible_flags(&file.content);
    if !removed.is_empty() {
        files.write_text(file.path, kept, Some(file.revision)).await?;
    }
    Ok(removed)
}

/// `text` without the flags, and the flags taken out.
fn without_incompatible_flags(text: &str) -> (String, Vec<String>) {
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
    (kept_lines.concat(), removed)
}

async fn from_run_script(text: &str, files: &ServerFiles) -> Result<LaunchSpec, InstallError> {
    let args_files = launch_line_args_files(text).ok_or_else(|| InstallError::LaunchNotFound {
        detail: format!("{RUN_SCRIPT} has no `java @…` launch line"),
    })?;
    for file in &args_files {
        if !is_inside(file) {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{RUN_SCRIPT} points outside the server folder: {file}"),
            });
        }
        if !is_file(files, file).await? {
            return Err(InstallError::LaunchNotFound {
                detail: format!("{RUN_SCRIPT} refers to {file}, which the installer did not create"),
            });
        }
    }
    let args_file = args_files.last().cloned().unwrap_or_default();
    let jvm_args_file = args_files
        .iter()
        .find(|file| file.ends_with(USER_JVM_ARGS))
        .cloned()
        .unwrap_or_else(|| USER_JVM_ARGS.to_owned());
    if jvm_args_file == args_file {
        return Err(InstallError::LaunchNotFound {
            detail: format!("{RUN_SCRIPT} names no arguments file besides {USER_JVM_ARGS}"),
        });
    }
    ensure_exists(files, &jvm_args_file).await?;
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
async fn legacy_forge_jar(files: &ServerFiles) -> Result<LaunchSpec, InstallError> {
    let mut jars: Vec<String> = files
        .list(SafeRelPath::root())
        .await?
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .map(|entry| entry.name)
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

/// A real file, not a link to one: the app reads and edits these.
async fn is_file(files: &ServerFiles, file: &str) -> Result<bool, InstallError> {
    Ok(files.kind(path(file)?).await? == Some(EntryKind::File))
}

/// Some installers only reference `user_jvm_args.txt`; an empty one keeps the launch valid.
async fn ensure_exists(files: &ServerFiles, file: &str) -> Result<(), InstallError> {
    if is_file(files, file).await? {
        return Ok(());
    }
    files.write_text(path(file)?, String::new(), None).await?;
    Ok(())
}

fn path(file: &str) -> Result<SafeRelPath, InstallError> {
    Ok(SafeRelPath::parse(file).map_err(FilesError::from)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture_text, server_files};

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

    async fn detect_in(dir: &Path) -> Result<LaunchSpec, InstallError> {
        detect(&server_files(dir).await).await
    }

    #[tokio::test]
    async fn reads_the_real_forge_26_3_run_script() {
        let args = "libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt";
        let dir = installed(&fixture_text("forge_26.3_run.sh"), &[USER_JVM_ARGS, args]);
        assert_eq!(
            detect_in(dir.path()).await.unwrap(),
            LaunchSpec::ArgsFiles {
                jvm_args_file: USER_JVM_ARGS.into(),
                args_file: args.into(),
                args: vec!["nogui".into()],
            }
        );
    }

    #[tokio::test]
    async fn reads_the_real_neoforge_26_3_run_script() {
        let args = "libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt";
        let dir = installed(&fixture_text("neoforge_26.3_run.sh"), &[USER_JVM_ARGS, args]);
        let LaunchSpec::ArgsFiles { args_file, .. } = detect_in(dir.path()).await.unwrap() else {
            panic!("expected args files");
        };
        assert_eq!(args_file, args);
    }

    #[tokio::test]
    async fn accepts_exec_and_a_full_java_path() {
        for line in [
            "exec java @user_jvm_args.txt @libraries/x/unix_args.txt \"$@\"",
            "\"/usr/lib/jvm/bin/java\" @user_jvm_args.txt @libraries/x/unix_args.txt",
        ] {
            let dir = installed(
                &format!("#!/bin/sh\n{line}\n"),
                &[USER_JVM_ARGS, "libraries/x/unix_args.txt"],
            );
            assert!(
                matches!(detect_in(dir.path()).await, Ok(LaunchSpec::ArgsFiles { .. })),
                "{line}"
            );
        }
    }

    #[tokio::test]
    async fn creates_a_missing_user_jvm_args_file() {
        let dir = installed(
            "exec java @libraries/x/unix_args.txt \"$@\"\n",
            &["libraries/x/unix_args.txt"],
        );
        assert!(matches!(
            detect_in(dir.path()).await.unwrap(),
            LaunchSpec::ArgsFiles { .. }
        ));
        assert!(dir.path().join(USER_JVM_ARGS).is_file());
    }

    #[tokio::test]
    async fn rejects_paths_that_escape_the_server_folder() {
        for evil in ["@../../etc/passwd", "@/etc/passwd", "@libraries/../../../x"] {
            let dir = installed(&format!("java @user_jvm_args.txt {evil}\n"), &[USER_JVM_ARGS]);
            assert!(
                matches!(
                    detect_in(dir.path()).await,
                    Err(InstallError::LaunchNotFound { .. })
                ),
                "{evil}"
            );
        }
    }

    /// The installer (downloaded code) may leave links: an args file that is one isn't used, and a
    /// missing `user_jvm_args.txt` isn't created through one.
    #[cfg(unix)]
    #[tokio::test]
    async fn links_the_installer_left_are_not_followed() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "-Djava.net.preferIPv6Addresses=system\n").unwrap();

        let linked_args = installed(
            "java @user_jvm_args.txt @libraries/x/unix_args.txt\n",
            &[USER_JVM_ARGS],
        );
        std::fs::create_dir_all(linked_args.path().join("libraries/x")).unwrap();
        symlink(&secret, linked_args.path().join("libraries/x/unix_args.txt")).unwrap();
        let error = detect_in(linked_args.path()).await.unwrap_err().to_string();
        assert!(error.contains("did not create"), "{error}");

        let created = outside.path().join("created-by-the-app");
        let linked_jvm_args = installed(
            "java @libraries/x/unix_args.txt\n",
            &["libraries/x/unix_args.txt"],
        );
        symlink(&created, linked_jvm_args.path().join(USER_JVM_ARGS)).unwrap();
        assert!(detect_in(linked_jvm_args.path()).await.is_err());
        assert!(!created.exists());

        let files = server_files(linked_args.path()).await;
        assert!(
            strip_incompatible_flags(&files, "libraries/x/unix_args.txt")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "-Djava.net.preferIPv6Addresses=system\n",
            "what the link points at is as it was"
        );
    }

    #[tokio::test]
    async fn rejects_scripts_without_a_usable_launch_line() {
        let cases = [
            "#!/bin/sh\necho hello\n",
            "# java @user_jvm_args.txt @libraries/x/unix_args.txt\n",
            "java -jar server.jar\n",
            "java @user_jvm_args.txt \"$@\"\n",
            "echo @libraries/x/unix_args.txt java\n",
        ];
        for script in cases {
            let dir = installed(script, &[USER_JVM_ARGS]);
            assert!(detect_in(dir.path()).await.is_err(), "{script:?}");
        }
        let missing = installed(
            "java @user_jvm_args.txt @libraries/gone/unix_args.txt\n",
            &[USER_JVM_ARGS],
        );
        let error = detect_in(missing.path()).await.unwrap_err().to_string();
        assert!(error.contains("did not create"), "{error}");
    }

    #[tokio::test]
    async fn strips_the_ipv6_flag_from_real_forge_and_neoforge_args_files() {
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
        let files = server_files(dir.path()).await;

        assert_eq!(
            strip_incompatible_flags(&files, "forge_unix_args.txt")
                .await
                .unwrap(),
            ["-Djava.net.preferIPv6Addresses=system"]
        );
        assert_eq!(
            std::fs::read_to_string(&forge).unwrap(),
            "-XX:+UseCompactObjectHeaders -XX:StackShadowPages=32 -jar forge-26.3-66.1.0-shim.jar\n"
        );
        assert_eq!(
            strip_incompatible_flags(&files, "neoforge_unix_args.txt")
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(&neoforge).unwrap(),
            "--add-opens java.base/java.lang.invoke=ALL-UNNAMED\n-DlibraryDirectory=libraries\n-classpath\nlibraries/a.jar:libraries/b.jar\nnet.neoforged.Main",
            "every other line is untouched, the last one without a newline too"
        );
    }

    #[tokio::test]
    async fn files_without_the_flag_are_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("unix_args.txt");
        std::fs::write(&file, "-Djava.net.preferIPv6Addresses=true -jar x.jar\n").unwrap();
        let files = server_files(dir.path()).await;
        assert_eq!(
            strip_incompatible_flags(&files, "unix_args.txt").await.unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "-Djava.net.preferIPv6Addresses=true -jar x.jar\n"
        );
    }

    #[tokio::test]
    async fn legacy_forge_launches_its_jar() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "forge-1.16.5-36.2.39.jar",
            "forge-installer.jar",
            "minecraft_server.1.16.5.jar",
        ] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        assert_eq!(
            detect_in(dir.path()).await.unwrap(),
            LaunchSpec::Jar {
                jar: "forge-1.16.5-36.2.39.jar".into(),
                args: vec!["nogui".into()],
            }
        );

        std::fs::write(dir.path().join("forge-1.16.5-36.2.38.jar"), "").unwrap();
        assert!(
            detect_in(dir.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("several")
        );
        assert!(detect_in(tempfile::tempdir().unwrap().path()).await.is_err());
    }
}
