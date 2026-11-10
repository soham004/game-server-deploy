//! Running the vanilla data generator for a server's Minecraft version: the exact command tree
//! (`reports/commands.json`) and the built-in registries (`reports/registries.json`).
//!
//! `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --output <dir>`
//! (checked on 26.3; jars before 1.18 have no bundler: `-cp server.jar net.minecraft.data.Main`).
//!
//! The vanilla jar comes from the shared download cache, as for installs (a modded server's version
//! is the same game underneath). It runs in the instance's sandbox, as its user, in a work folder in
//! the instance's `tmp/`, so it gets no more than the server has; the folder goes when it is done.
//! Its reports are read through the file manager's rules (no links out). The data-driven ids are
//! read from the cached jar itself, which only the app can write.

use std::{path::Path, time::Duration};

use chrono::Utc;
use gsd_core::{
    files::{SafeRelPath, ServerFiles},
    instances::{Instance, launch::helper_process},
    java::JavaRuntimes,
    jobs::JobContext,
    random::random_token,
    supervisor::{
        HostSandbox,
        process::{RunOutcome, run_to_completion},
    },
};
use serde_json::Value;
use tokio_util::io::ReaderStream;

use super::{CommandsError, VanillaCommands, registries::Registries, tree::CommandTree};
use crate::{catalog::Catalog, install, loader::LoaderKind};

/// Longest the generator may take (about 15 seconds on 26.3, most of it unpacking).
const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The server jar is about 60 MB.
const MAX_JAR_BYTES: u64 = 1024 * 1024 * 1024;
const OUTPUT: &str = "generated";

/// What running the generator needs besides the instance.
#[derive(Debug, Clone, Copy)]
pub struct Generator<'a> {
    pub catalog: &'a Catalog,
    pub java: &'a JavaRuntimes,
    pub host: &'a HostSandbox,
    /// The shared download cache (`<data>/cache/downloads`).
    pub download_cache: &'a Path,
}

/// The command tree and registries of `mc_version`, generated in `instance`'s sandbox.
pub async fn generate(
    generator: Generator<'_>,
    instance: &Instance,
    mc_version: &str,
    job: &JobContext,
) -> Result<VanillaCommands, CommandsError> {
    job.step(format!("Getting the Minecraft {mc_version} server"));
    let detail = generator.catalog.version_detail(mc_version).await?;
    let artifact = generator
        .catalog
        .artifact(LoaderKind::Vanilla, mc_version, None)
        .await?;
    let jar = install::cached_download(generator.catalog, &artifact, generator.download_cache, job).await?;
    let java = generator.java.ensure(detail.java_major(), job).await?;
    let bundled = {
        let jar = jar.clone();
        tokio::task::spawn_blocking(move || is_bundler(&jar))
            .await
            .map_err(|_| CommandsError::Failed("reading the server jar stopped".into()))??
    };

    generator.host.ensure_usable()?;
    let tmp = ServerFiles::open_tmp(instance).await?;
    let work = SafeRelPath::parse(&format!("gsd-commands-{}", random_token::<8>()?))?;
    tmp.create_dir(work.clone()).await?;
    let generated = run_generator(generator, instance, &tmp, &work, &jar, &java, bundled, job).await;
    if let Err(error) = tmp.remove_tree(work).await {
        job.log(format!("Could not remove the generator's work folder: {error}"));
    }
    let (commands, registries_report) = generated?;

    job.step("Reading the commands");
    let tree = CommandTree::from_report(&commands)?;
    let wanted = Registries::wanted(&tree);
    let mut registries = Registries::default();
    if let Some(report) = registries_report {
        registries.add_report(&report, &wanted);
    }
    let registries = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&jar)?;
        registries.add_from_jar(std::io::BufReader::new(file), &wanted)?;
        Ok::<_, CommandsError>(registries)
    })
    .await
    .map_err(|_| CommandsError::Failed("reading the server jar stopped".into()))??;
    job.log(format!(
        "{} commands, {} lists of ids",
        tree.commands().count(),
        registries.len()
    ));
    Ok(VanillaCommands {
        format: VanillaCommands::FORMAT,
        mc_version: mc_version.to_owned(),
        generated_at: Utc::now(),
        tree,
        registries,
    })
}

/// Copies the jar into the work folder, runs the generator there, and reads its reports.
#[allow(clippy::too_many_arguments)]
async fn run_generator(
    generator: Generator<'_>,
    instance: &Instance,
    tmp: &ServerFiles,
    work: &SafeRelPath,
    jar: &Path,
    java: &gsd_core::java::JavaRuntime,
    bundled: bool,
    job: &JobContext,
) -> Result<(Value, Option<Value>), CommandsError> {
    job.step("Running the data generator");
    let file = tokio::fs::File::open(jar)
        .await
        .map_err(|source| CommandsError::Io {
            path: jar.to_owned(),
            source,
        })?;
    tmp.upload(
        work.join("server.jar")?,
        ReaderStream::new(file),
        false,
        MAX_JAR_BYTES,
    )
    .await?;

    let mut args: Vec<String> = if bundled {
        ["-DbundlerMainClass=net.minecraft.data.Main", "-jar", "server.jar"]
            .map(str::to_owned)
            .to_vec()
    } else {
        ["-cp", "server.jar", "net.minecraft.data.Main"]
            .map(str::to_owned)
            .to_vec()
    };
    args.extend(["--reports", "--output", OUTPUT].map(str::to_owned));
    let cwd = instance.layout.tmp_dir().join(work.as_path());
    let spec = helper_process(&instance.definition, &instance.layout, java, cwd, &args);
    let run = run_to_completion(&spec, generator.host, job.cancel_token(), |line| job.log(line));
    let outcome = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| {
            CommandsError::Failed(format!(
                "the data generator took longer than {} minutes",
                TIMEOUT.as_secs() / 60
            ))
        })?
        .map_err(|source| CommandsError::Failed(format!("the data generator could not start: {source}")))?;
    match outcome {
        RunOutcome::Exited(exit) if exit.success() => {}
        RunOutcome::Exited(exit) => {
            return Err(CommandsError::Failed(format!(
                "the data generator failed (exit code {}); its output is in the job log",
                exit.code.map_or_else(|| "none".into(), |code| code.to_string())
            )));
        }
        RunOutcome::Cancelled => return Err(CommandsError::Cancelled),
    }

    let reports = work.join(OUTPUT)?.join("reports")?;
    let commands = match tmp.read_bytes(reports.join("commands.json")?).await {
        Ok((bytes, _)) => serde_json::from_slice(&bytes)
            .map_err(|error| CommandsError::Failed(format!("commands.json isn't JSON: {error}")))?,
        Err(gsd_core::files::FilesError::NotFound(_)) => {
            return Err(CommandsError::Unavailable(
                "this version's data generator has no command list (Minecraft 1.13 and later have one)"
                    .into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    // Older generators have no registries report; ids then come from the jar alone.
    let registries = match tmp.read_bytes(reports.join("registries.json")?).await {
        Ok((bytes, _)) => serde_json::from_slice(&bytes).ok(),
        Err(_) => None,
    };
    Ok((commands, registries))
}

/// Since 1.18 the server jar is a bundler (`META-INF/versions.list` lists the game inside it).
fn is_bundler(jar: &Path) -> Result<bool, CommandsError> {
    let file = std::fs::File::open(jar).map_err(|source| CommandsError::Io {
        path: jar.to_owned(),
        source,
    })?;
    let archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    Ok(archive.index_for_name("META-INF/versions.list").is_some())
}
