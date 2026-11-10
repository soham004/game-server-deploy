//! Runs an [`InstallPlan`] in an instance's server directory, then resolves its launch.

use std::{path::Path, time::Duration};

use gsd_core::{
    instances::{definition::LaunchSpec, launch::installer_process},
    jobs::JobContext,
    net::{DownloadError, ExpectedHash, download_verified},
    supervisor::process::{RunOutcome, run_to_completion},
};

use super::{InstallError, InstallTarget, LaunchPlan, plan::InstallStep, run_script};
use crate::catalog::{Artifact, Catalog};

/// Longest an installer may run (Forge on a slow disk and network takes a few minutes).
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Executes every step in order, stopping at the first failure or cancellation.
pub async fn run_steps(
    catalog: &Catalog,
    target: &InstallTarget<'_>,
    steps: &[InstallStep],
    job: &JobContext,
) -> Result<(), InstallError> {
    let server_dir = target.instance.layout.server_dir();
    for step in steps {
        job.check_cancelled().map_err(|_| InstallError::Cancelled)?;
        match step {
            InstallStep::Download {
                label,
                artifact,
                file,
            } => {
                job.step(label);
                download(catalog, artifact, &server_dir.join(file), target.cache_dir, job).await?;
            }
            InstallStep::RunInstaller { label, jar, args } => {
                job.step(label);
                run_installer(target, jar, args, job).await?;
            }
            InstallStep::Remove { files } => remove(&server_dir, files).await?,
        }
    }
    Ok(())
}

/// The finished server's launch. Run scripts are read, then deleted (they'd start the server
/// outside the sandbox and with the wrong Java if someone ran them); args files lose the JVM
/// flags that break servers in the sandbox.
pub async fn resolve_launch(
    plan: &LaunchPlan,
    server_dir: &Path,
    job: &JobContext,
) -> Result<LaunchSpec, InstallError> {
    let LaunchPlan::Jar(jar) = plan else {
        let launch = run_script::detect(server_dir)?;
        if let LaunchSpec::ArgsFiles { args_file, .. } = &launch {
            for flag in run_script::strip_incompatible_flags(&server_dir.join(args_file))? {
                job.log(format!(
                    "Removed {flag} from {args_file}: it breaks binding the port in the sandbox"
                ));
            }
        }
        let scripts: Vec<String> = run_script::SCRIPTS.iter().map(|s| (*s).to_owned()).collect();
        remove(server_dir, &scripts).await?;
        return Ok(launch);
    };
    Ok(LaunchSpec::Jar {
        jar: jar.clone(),
        args: vec!["nogui".into()],
    })
}

/// Downloads into the shared cache (keyed by checksum) and copies from there, so a second
/// server with the same jar costs no download. Unverifiable files go straight to `destination`.
async fn download(
    catalog: &Catalog,
    artifact: &Artifact,
    destination: &Path,
    cache_dir: &Path,
    job: &JobContext,
) -> Result<(), InstallError> {
    if cache_key(&artifact.hash).is_none() {
        return fetch(catalog, artifact, destination, job).await;
    }
    let cached = cached_download(catalog, artifact, cache_dir, job).await?;
    tokio::fs::copy(&cached, destination)
        .await
        .map_err(|source| InstallError::Io {
            path: destination.to_owned(),
            source,
        })?;
    Ok(())
}

/// The artifact in the shared cache, downloaded (and verified) first if it isn't there yet. Only for
/// artifacts with a checksum: the cache is keyed by it.
pub(crate) async fn cached_download(
    catalog: &Catalog,
    artifact: &Artifact,
    cache_dir: &Path,
    job: &JobContext,
) -> Result<std::path::PathBuf, InstallError> {
    let key = cache_key(&artifact.hash).ok_or_else(|| InstallError::LaunchNotFound {
        detail: format!("{} has no checksum to cache it by", artifact.url),
    })?;
    let cached = cache_dir.join(format!("{key}.jar"));
    if tokio::fs::try_exists(&cached).await.unwrap_or(false) {
        job.log(format!("Using cached {}", cached.display()));
    } else {
        fetch(catalog, artifact, &cached, job).await?;
    }
    Ok(cached)
}

async fn fetch(
    catalog: &Catalog,
    artifact: &Artifact,
    destination: &Path,
    job: &JobContext,
) -> Result<(), InstallError> {
    job.log(format!("Downloading {}", artifact.url));
    let progress = |done: u64, total: Option<u64>| {
        if let Some(total) = total.filter(|total| *total > 0) {
            #[allow(clippy::cast_precision_loss)]
            job.progress(done as f32 / total as f32);
        }
    };
    download_verified(
        catalog.client(),
        &artifact.url,
        destination,
        &artifact.hash,
        job.cancel_token(),
        &progress,
    )
    .await
    .map_err(|error| match error {
        DownloadError::Cancelled => InstallError::Cancelled,
        other => InstallError::Download(other),
    })
}

/// The checksum as a cache file name (`None` for unverifiable artifacts).
fn cache_key(hash: &ExpectedHash) -> Option<String> {
    let hex = match hash {
        ExpectedHash::Md5(hex)
        | ExpectedHash::Sha1(hex)
        | ExpectedHash::Sha256(hex)
        | ExpectedHash::Sha512(hex) => hex,
        ExpectedHash::None => return None,
    };
    Some(hex.to_ascii_lowercase()).filter(|hex| hex.chars().all(|c| c.is_ascii_hexdigit()) && !hex.is_empty())
}

/// Runs a loader installer in the instance's sandbox; its output goes to the job log.
async fn run_installer(
    target: &InstallTarget<'_>,
    jar: &str,
    args: &[String],
    job: &JobContext,
) -> Result<(), InstallError> {
    target.host.ensure_usable()?;
    let instance = target.instance;
    // The installer runs as the instance's user: it must own the files it extends.
    instance
        .layout
        .assign_ownership(instance.definition.sandbox)
        .map_err(|source| InstallError::Io {
            path: instance.layout.server_dir(),
            source,
        })?;
    let spec = installer_process(&instance.definition, &instance.layout, target.java, jar, args);
    let run = run_to_completion(&spec, target.host, job.cancel_token(), |line| job.log(line));
    let outcome = tokio::time::timeout(INSTALLER_TIMEOUT, run)
        .await
        .map_err(|_| InstallError::InstallerTimedOut {
            minutes: INSTALLER_TIMEOUT.as_secs() / 60,
        })?
        .map_err(|source| InstallError::InstallerDidNotStart { source })?;
    match outcome {
        RunOutcome::Exited(exit) if exit.success() => Ok(()),
        RunOutcome::Exited(exit) => Err(InstallError::InstallerFailed { code: exit.code }),
        RunOutcome::Cancelled => Err(InstallError::Cancelled),
    }
}

async fn remove(server_dir: &Path, files: &[String]) -> Result<(), InstallError> {
    for file in files {
        let path = server_dir.join(file);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(InstallError::Io { path, source }),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_are_lowercase_hex_only() {
        assert_eq!(
            cache_key(&ExpectedHash::Sha1("ABCdef01".into())).as_deref(),
            Some("abcdef01")
        );
        assert_eq!(cache_key(&ExpectedHash::Md5("0f".into())).as_deref(), Some("0f"));
        assert_eq!(cache_key(&ExpectedHash::None), None);
        assert_eq!(
            cache_key(&ExpectedHash::Sha256("../../etc".into())),
            None,
            "never a path"
        );
        assert_eq!(cache_key(&ExpectedHash::Sha256(String::new())), None);
    }

    #[tokio::test]
    async fn removing_missing_files_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jar"), "").unwrap();
        remove(dir.path(), &["a.jar".into(), "missing.log".into()])
            .await
            .unwrap();
        assert!(!dir.path().join("a.jar").exists());
    }
}
