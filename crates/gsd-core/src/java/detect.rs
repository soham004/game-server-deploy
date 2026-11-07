//! Finding Java installations on this machine.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use super::JavaRuntime;

const EXECUTABLE: &str = if cfg!(windows) { "java.exe" } else { "java" };

/// Scans managed runtimes and (optionally) common system locations. Managed runtimes come first.
pub(super) fn scan(managed_dir: &Path, scan_system: bool) -> Vec<JavaRuntime> {
    let mut candidates: Vec<(PathBuf, bool)> = subdirectories(managed_dir)
        .into_iter()
        .map(|home| (home, true))
        .collect();
    if scan_system {
        candidates.extend(system_homes().into_iter().map(|home| (home, false)));
    }

    let mut seen = HashSet::new();
    let mut runtimes = Vec::new();
    for (home, managed) in candidates {
        let Ok(canonical) = dunce_canonicalize(&home) else {
            continue;
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if let Some(runtime) = inspect(&canonical, managed) {
            runtimes.push(runtime);
        }
    }
    runtimes.sort_by(|a, b| b.managed.cmp(&a.managed).then(b.major.cmp(&a.major)));
    runtimes
}

/// Reads one Java home: version from its `release` file, or by asking the binary.
fn inspect(home: &Path, managed: bool) -> Option<JavaRuntime> {
    let executable = home.join("bin").join(EXECUTABLE);
    if !executable.is_file() {
        return None;
    }
    let (version, vendor) = std::fs::read_to_string(home.join("release"))
        .ok()
        .and_then(|text| parse_release_file(&text))
        .or_else(|| query_version(&executable).map(|version| (version, None)))?;
    let major = parse_major(&version)?;
    let id = if managed {
        home.file_name().map_or_else(
            || home.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    } else {
        home.display().to_string()
    };
    Some(JavaRuntime {
        id,
        home: home.to_owned(),
        executable,
        major,
        version,
        vendor,
        managed,
    })
}

/// Parses `JAVA_VERSION="21.0.5"` and `IMPLEMENTOR="Eclipse Adoptium"` from a `release` file.
pub fn parse_release_file(text: &str) -> Option<(String, Option<String>)> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
        })
    };
    Some((value("JAVA_VERSION")?, value("IMPLEMENTOR")))
}

/// The major version from a Java version string: `1.8.0_402` → 8, `25.0.1` → 25, `17` → 17.
pub fn parse_major(version: &str) -> Option<u32> {
    let mut parts = version
        .trim()
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty());
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Fallback for homes without a `release` file: `java -XshowSettings:properties -version`.
fn query_version(executable: &Path) -> Option<String> {
    let mut child = Command::new(executable)
        .args(["-XshowSettings:properties", "-version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    // Don't let a broken binary hang detection.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| line.trim().strip_prefix("java.version = ").map(str::to_owned))
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn system_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(java_home) = std::env::var_os("JAVA_HOME") {
        homes.push(PathBuf::from(java_home));
    }
    // `java` on PATH: resolve symlinks (e.g. /usr/bin/java → /usr/lib/jvm/…/bin/java).
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(EXECUTABLE);
            if let Ok(real) = dunce_canonicalize(&candidate)
                && let Some(home) = real.parent().and_then(Path::parent)
            {
                homes.push(home.to_owned());
            }
        }
    }
    for root in platform_roots() {
        homes.extend(subdirectories(Path::new(root)));
    }
    if cfg!(target_os = "macos") {
        homes = homes.into_iter().map(|home| mac_home(&home)).collect();
    }
    homes
}

fn platform_roots() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files\BellSoft",
        ]
    } else if cfg!(target_os = "macos") {
        &["/Library/Java/JavaVirtualMachines"]
    } else {
        &["/usr/lib/jvm", "/opt/java", "/usr/java"]
    }
}

/// macOS bundles keep the real home under `Contents/Home`.
fn mac_home(path: &Path) -> PathBuf {
    let inner = path.join("Contents/Home");
    if inner.is_dir() { inner } else { path.to_owned() }
}

/// `canonicalize` without Windows `\\?\` prefixes.
fn dunce_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = canonical.display().to_string();
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(stripped));
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_major_versions() {
        assert_eq!(parse_major("1.8.0_402"), Some(8));
        assert_eq!(parse_major("17.0.12"), Some(17));
        assert_eq!(parse_major("21"), Some(21));
        assert_eq!(parse_major("25-ea"), Some(25));
        assert_eq!(parse_major("garbage"), None);
    }

    #[test]
    fn parses_release_files() {
        let text = "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"25.0.1\"\nOS_ARCH=\"x86_64\"\n";
        assert_eq!(
            parse_release_file(text),
            Some(("25.0.1".into(), Some("Eclipse Adoptium".into())))
        );
        assert_eq!(
            parse_release_file("JAVA_VERSION=\"21.0.5\""),
            Some(("21.0.5".into(), None))
        );
        assert_eq!(parse_release_file("OTHER=1"), None);
    }

    #[test]
    fn scans_homes_with_release_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("temurin-21-x64");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join(EXECUTABLE), "").unwrap();
        std::fs::write(
            home.join("release"),
            "JAVA_VERSION=\"21.0.5\"\nIMPLEMENTOR=\"Eclipse Adoptium\"",
        )
        .unwrap();
        // A directory without bin/java is ignored.
        std::fs::create_dir_all(dir.path().join("not-java")).unwrap();

        let found = scan(dir.path(), false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "temurin-21-x64");
        assert_eq!(found[0].major, 21);
        assert!(found[0].managed);
    }
}
