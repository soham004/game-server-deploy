//! Downloading Eclipse Temurin runtimes from the Adoptium API.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{JavaError, JavaRuntime, detect};
use crate::{
    jobs::JobContext,
    net::{ExpectedHash, download_verified},
};

pub const ADOPTIUM_API: &str = "https://api.adoptium.net";

#[derive(Debug, Deserialize)]
struct Asset {
    binary: Binary,
}

#[derive(Debug, Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    link: String,
    checksum: String,
    name: String,
}

/// Downloads and unpacks Temurin `major` into `managed_dir`; returns the new runtime.
pub(super) async fn install(
    client: &reqwest::Client,
    api_base: &str,
    managed_dir: &Path,
    major: u32,
    job: &JobContext,
) -> Result<JavaRuntime, JavaError> {
    let (os, arch) = platform();
    job.step(format!("Finding Java {major} for {os}/{arch}"));
    let package = find_package(client, api_base, major, os, arch).await?;

    let downloads = managed_dir.join(".downloads");
    let archive = downloads.join(&package.name);
    job.step(format!("Downloading Java {major}"));
    download_verified(
        client,
        &package.link,
        &archive,
        &ExpectedHash::Sha256(package.checksum.clone()),
        job.cancel_token(),
        &|done, total| {
            if let Some(total) = total.filter(|t| *t > 0) {
                #[allow(clippy::cast_precision_loss)]
                job.progress(done as f32 / total as f32);
            }
        },
    )
    .await?;

    job.step(format!("Unpacking Java {major}"));
    let target = managed_dir.join(format!("temurin-{major}-{arch}"));
    let (archive_path, target_path) = (archive.clone(), target.clone());
    let home = tokio::task::spawn_blocking(move || unpack(&archive_path, &target_path))
        .await
        .map_err(|e| JavaError::Extract(std::io::Error::other(e)))??;
    let _ = tokio::fs::remove_file(&archive).await;

    let runtime = detect::scan(managed_dir, false)
        .into_iter()
        .find(|runtime| runtime.home == home || runtime.home == target)
        .ok_or(JavaError::NoExecutable)?;
    job.log(format!(
        "Installed {} {} at {}",
        runtime.vendor.as_deref().unwrap_or("Java"),
        runtime.version,
        runtime.home.display()
    ));
    Ok(runtime)
}

async fn find_package(
    client: &reqwest::Client,
    api_base: &str,
    major: u32,
    os: &'static str,
    arch: &'static str,
) -> Result<Package, JavaError> {
    // Prefer the smaller JRE; fall back to a JDK where no JRE is published.
    for image_type in ["jre", "jdk"] {
        let url = format!("{api_base}/v3/assets/latest/{major}/hotspot");
        let response = client
            .get(url)
            .query(&[
                ("os", os),
                ("architecture", arch),
                ("image_type", image_type),
                ("vendor", "eclipse"),
            ])
            .send()
            .await
            .map_err(JavaError::Api)?;
        if !response.status().is_success() {
            continue;
        }
        let assets: Vec<Asset> = response.json().await.map_err(JavaError::Api)?;
        if let Some(asset) = assets.into_iter().next() {
            return Ok(asset.binary.package);
        }
    }
    Err(JavaError::NotAvailable { major, os, arch })
}

/// Unpacks into a temporary sibling directory, then moves the Java home into `target`.
fn unpack(archive: &Path, target: &Path) -> Result<PathBuf, JavaError> {
    let parent = target.parent().unwrap_or(target);
    let staging = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(parent)
        .map_err(|source| JavaError::Io {
            path: parent.to_owned(),
            source,
        })?;

    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let file = std::fs::File::open(archive).map_err(JavaError::Extract)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| JavaError::Extract(std::io::Error::other(e)))?;
        zip.extract(staging.path())
            .map_err(|e| JavaError::Extract(std::io::Error::other(e)))?;
    } else {
        let file = std::fs::File::open(archive).map_err(JavaError::Extract)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(staging.path()).map_err(JavaError::Extract)?;
    }

    let home = find_home(staging.path()).ok_or(JavaError::NoExecutable)?;
    if target.exists() {
        std::fs::remove_dir_all(target).map_err(|source| JavaError::Io {
            path: target.to_owned(),
            source,
        })?;
    }
    std::fs::rename(&home, target).map_err(|source| JavaError::Io {
        path: target.to_owned(),
        source,
    })?;
    make_world_readable(target)?;
    Ok(target.to_owned())
}

/// The directory (at most three levels down) that contains `bin/java`.
fn find_home(root: &Path) -> Option<PathBuf> {
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    let mut frontier = vec![(root.to_owned(), 0)];
    while let Some((dir, depth)) = frontier.pop() {
        if dir.join("bin").join(executable).is_file() {
            return Some(dir);
        }
        if depth < 3 {
            let children = std::fs::read_dir(&dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|e| e.path());
            frontier.extend(children.filter(|p| p.is_dir()).map(|p| (p, depth + 1)));
        }
    }
    None
}

/// Sandboxed servers run as other users and must be able to read and execute the runtime.
fn make_world_readable(dir: &Path) -> Result<(), JavaError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| JavaError::Io { path, source }
        };
        let mut stack = vec![dir.to_owned()];
        while let Some(path) = stack.pop() {
            let metadata = std::fs::symlink_metadata(&path).map_err(io(&path))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let mode = metadata.permissions().mode();
            let wanted = if metadata.is_dir() || mode & 0o111 != 0 {
                mode | 0o555
            } else {
                mode | 0o444
            };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wanted & !0o022))
                .map_err(io(&path))?;
            if metadata.is_dir() {
                for entry in std::fs::read_dir(&path).map_err(io(&path))? {
                    stack.push(entry.map_err(io(&path))?.path());
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "mac",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        other => other,
    };
    (os, arch)
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::{io::Write, os::unix::fs::PermissionsExt};

    use sha2::{Digest, Sha256};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    use super::*;
    use crate::{
        events::EventBus,
        java::JavaRuntimes,
        jobs::{JobRunner, JobSpec, JobStatus},
        net::http_client,
    };

    /// A tar.gz shaped like a Temurin JRE: `jdk-21.0.5+11-jre/{bin/java,release}`.
    fn fake_temurin_archive() -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut add = |path: &str, mode: u32, body: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            builder.append_data(&mut header, path, body).unwrap();
        };
        add(
            "jdk-21.0.5+11-jre/bin/java",
            0o700,
            b"#!/bin/sh\necho fake java\n",
        );
        add(
            "jdk-21.0.5+11-jre/release",
            0o600,
            b"JAVA_VERSION=\"21.0.5\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\n",
        );
        let mut gz = builder.into_inner().unwrap();
        gz.flush().unwrap();
        gz.finish().unwrap()
    }

    #[tokio::test]
    async fn installs_temurin_from_the_api() {
        let server = MockServer::start().await;
        let archive = fake_temurin_archive();
        let checksum = hex::encode(Sha256::digest(&archive));
        let (os, arch) = platform();
        Mock::given(method("GET"))
            .and(path("/v3/assets/latest/21/hotspot"))
            .and(query_param("image_type", "jre"))
            .and(query_param("os", os))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "binary": { "package": {
                    "link": format!("{}/download/jre21.tar.gz", server.uri()),
                    "checksum": checksum,
                    "name": "jre21.tar.gz",
                }}
            }])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/jre21.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let runtimes =
            JavaRuntimes::with_api(dir.path().to_owned(), http_client().unwrap(), server.uri(), false);
        let jobs = JobRunner::new(EventBus::new());
        let installer = runtimes.clone();
        let id = jobs
            .spawn(
                JobSpec {
                    kind: "java.install",
                    title: "Java".into(),
                    instance_id: None,
                },
                move |ctx| async move {
                    let runtime = installer.ensure(21, &ctx).await?;
                    assert_eq!(runtime.major, 21);
                    Ok(())
                },
            )
            .unwrap();
        let finished = jobs.wait(id).await.unwrap();
        assert_eq!(finished.status, JobStatus::Succeeded, "{:?}", finished.error);

        let runtime = runtimes.find(21).await.unwrap();
        assert!(runtime.managed);
        assert_eq!(runtime.id, format!("temurin-21-{arch}"));
        assert_eq!(runtime.vendor.as_deref(), Some("Eclipse Adoptium"));
        // Other users (sandboxed servers) can execute it.
        let mode = std::fs::metadata(&runtime.executable)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o005, 0o005, "mode {mode:o}");
        assert!(
            !dir.path().join(".downloads/jre21.tar.gz").exists(),
            "archive cleaned up"
        );
    }

    #[tokio::test]
    async fn reports_unavailable_versions() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let result = find_package(&http_client().unwrap(), &server.uri(), 99, "linux", "x64").await;
        assert!(matches!(result, Err(JavaError::NotAvailable { major: 99, .. })));
        drop(dir);
    }

    #[test]
    fn finds_nested_homes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("jdk/Contents/Home");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin/java"), "").unwrap();
        assert_eq!(find_home(dir.path()), Some(home));
        assert_eq!(find_home(&dir.path().join("jdk/Contents/Home/bin")), None);
    }
}
