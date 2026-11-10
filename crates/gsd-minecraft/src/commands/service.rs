//! [`CommandService`]: each server's commands, prepared when first wanted and kept in its folder.
//!
//! - `<instance>/commands/vanilla.json`: the data generator's tree and ids for the server's version,
//!   made by a background job (one at a time on the machine; a JVM each) and made again after a
//!   version change.
//! - `<instance>/commands/learned.json`: what its `help` said, asked again when the mods or plugins
//!   change. Vanilla servers don't need it, unless the generator couldn't run.
//!
//! Both are prepared when someone who may type commands opens the console ([`CommandService::prepare`]),
//! not for every server at startup.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::Utc;
use gsd_core::{
    events::EventBus,
    ids::{InstanceId, JobId},
    instances::{Instance, InstanceService},
    java::JavaRuntimes,
    jobs::{JobFailure, JobRunner, JobSpec, error_chain},
    supervisor::{HostSandbox, ServerState},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Semaphore;

use super::{
    CommandsError, LearnedCommands, VanillaCommands,
    complete::{self, Completion, Sources},
    datagen::{self, Generator},
    help, learn,
    registries::Registries,
    tree::CommandTree,
};
use crate::{catalog::Catalog, loader::LoaderKind, settings::MinecraftServer};

const FOLDER: &str = "commands";
const VANILLA_FILE: &str = "vanilla.json";
const LEARNED_FILE: &str = "learned.json";
/// The saved files are a few hundred kilobytes; anything far bigger isn't ours.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the vanilla commands stand for one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VanillaState {
    Ready {
        mc_version: String,
        commands: usize,
    },
    Preparing {
        job_id: JobId,
    },
    /// The generator failed, or the version has none; `prepare` with `retry` tries again.
    Unavailable {
        reason: String,
    },
    /// Not prepared yet.
    Missing,
}

/// Where the commands learned from `help` stand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LearnedState {
    /// A vanilla server whose generator ran: `help` would say nothing new.
    NotNeeded,
    Current,
    Learning,
    /// Learned with other mods or plugins (or never); asked when the server runs.
    Stale,
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandsStatus {
    pub vanilla: VanillaState,
    pub learned: LearnedState,
    /// Commands learned from `help` that the generator's tree doesn't have.
    pub learned_commands: usize,
}

#[derive(Debug, Clone)]
pub struct CommandService {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    instances: InstanceService,
    events: EventBus,
    jobs: JobRunner,
    catalog: Catalog,
    java: JavaRuntimes,
    host: HostSandbox,
    download_cache: PathBuf,
    /// One generator at a time: each is a JVM with a 1 GiB heap.
    generating: Semaphore,
    servers: Mutex<HashMap<InstanceId, Entry>>,
}

/// What's known about one server's commands, besides its files.
#[derive(Debug, Default)]
struct Entry {
    /// The tree with the learned commands in it, and the ids, as last read.
    loaded: Option<Arc<Loaded>>,
    preparing: Option<JobId>,
    /// The version the generator failed for, and why.
    failed: Option<(String, String)>,
    learning: bool,
    /// The mods and plugins `help` failed with, and why.
    learn_failed: Option<(String, String)>,
}

#[derive(Debug)]
struct Loaded {
    mc_version: String,
    tree: CommandTree,
    registries: Registries,
    vanilla_commands: Option<usize>,
    learned_fingerprint: Option<String>,
    learned_added: usize,
}

impl CommandService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instances: InstanceService,
        events: EventBus,
        jobs: JobRunner,
        catalog: Catalog,
        java: JavaRuntimes,
        host: HostSandbox,
        download_cache: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                instances,
                events,
                jobs,
                catalog,
                java,
                host,
                download_cache,
                generating: Semaphore::new(1),
                servers: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Where things stand, without starting anything.
    pub async fn status(&self, id: InstanceId) -> Result<CommandsStatus, CommandsError> {
        let instance = self.inner.instances.get(id).await?;
        let server = MinecraftServer::from_table(&instance.definition.server)?;
        self.status_of(&instance, &server).await
    }

    /// Starts what's missing: the generator for this version, and `help` if the server runs and
    /// its mods or plugins changed since it was last asked. `retry` forgets earlier failures.
    pub async fn prepare(&self, id: InstanceId, retry: bool) -> Result<CommandsStatus, CommandsError> {
        let instance = self.inner.instances.get(id).await?;
        let server = MinecraftServer::from_table(&instance.definition.server)?;
        if retry {
            let mut servers = self.servers();
            let entry = servers.entry(id).or_default();
            entry.failed = None;
            entry.learn_failed = None;
        }
        let status = self.status_of(&instance, &server).await?;
        if status.vanilla == VanillaState::Missing && !self.inner.jobs.is_instance_busy(id) {
            self.start_generator(instance.clone(), server.mc_version.clone())?;
        }
        if status.learned == LearnedState::Stale
            && matches!(self.inner.instances.state(id), ServerState::Running { .. })
        {
            self.start_learning(instance.clone(), server.clone()).await?;
        }
        self.status_of(&instance, &server).await
    }

    /// What can come next at `cursor` in `input` (offsets in UTF-16 units).
    pub async fn complete(
        &self,
        id: InstanceId,
        input: &str,
        cursor: usize,
    ) -> Result<Completion, CommandsError> {
        let instance = self.inner.instances.get(id).await?;
        let server = MinecraftServer::from_table(&instance.definition.server)?;
        let loaded = self.loaded(&instance, &server).await?;
        let players = self.inner.instances.players(id).await.unwrap_or_default();
        let sources = Sources {
            players: &players,
            registries: &loaded.registries,
        };
        Ok(complete::complete(&loaded.tree, sources, input, cursor))
    }

    async fn status_of(
        &self,
        instance: &Instance,
        server: &MinecraftServer,
    ) -> Result<CommandsStatus, CommandsError> {
        let id = instance.id();
        let loaded = self.loaded(instance, server).await?;
        let fingerprint = learn::fingerprint(instance, server).await?;
        let servers = self.servers();
        let entry = servers.get(&id);
        let vanilla = match (&loaded.vanilla_commands, entry) {
            (Some(commands), _) => VanillaState::Ready {
                mc_version: loaded.mc_version.clone(),
                commands: *commands,
            },
            (
                None,
                Some(Entry {
                    preparing: Some(job_id),
                    ..
                }),
            ) => VanillaState::Preparing { job_id: *job_id },
            (
                None,
                Some(Entry {
                    failed: Some((mc, reason)),
                    ..
                }),
            ) if *mc == server.mc_version => VanillaState::Unavailable {
                reason: reason.clone(),
            },
            (None, _) => VanillaState::Missing,
        };
        let needs_help =
            server.loader != LoaderKind::Vanilla || matches!(vanilla, VanillaState::Unavailable { .. });
        let learned = if entry.is_some_and(|entry| entry.learning) {
            LearnedState::Learning
        } else if !needs_help {
            LearnedState::NotNeeded
        } else if loaded.learned_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            LearnedState::Current
        } else if let Some((failed_with, reason)) = entry.and_then(|entry| entry.learn_failed.as_ref())
            && *failed_with == fingerprint
        {
            LearnedState::Failed {
                reason: reason.clone(),
            }
        } else {
            LearnedState::Stale
        };
        Ok(CommandsStatus {
            vanilla,
            learned,
            learned_commands: loaded.learned_added,
        })
    }

    /// The tree and ids, from memory or the server's files. A vanilla tree for another version
    /// (the server's was changed) counts as none.
    async fn loaded(
        &self,
        instance: &Instance,
        server: &MinecraftServer,
    ) -> Result<Arc<Loaded>, CommandsError> {
        let id = instance.id();
        if let Some(loaded) = self.servers().get(&id).and_then(|entry| entry.loaded.clone())
            && loaded.mc_version == server.mc_version
        {
            return Ok(loaded);
        }
        let folder = folder(instance);
        let vanilla: Option<VanillaCommands> =
            read_json(&folder.join(VANILLA_FILE))
                .await?
                .filter(|vanilla: &VanillaCommands| {
                    vanilla.format == VanillaCommands::FORMAT && vanilla.mc_version == server.mc_version
                });
        let learned: Option<LearnedCommands> = read_json(&folder.join(LEARNED_FILE))
            .await?
            .filter(|learned: &LearnedCommands| learned.format == LearnedCommands::FORMAT);
        let vanilla_commands = vanilla.as_ref().map(|vanilla| vanilla.tree.commands().count());
        let (mut tree, registries) = vanilla.map_or_else(
            || (CommandTree::empty(), Registries::default()),
            |vanilla| (vanilla.tree, vanilla.registries),
        );
        let learned_added = match &learned {
            Some(learned) => tree.learn(&learned.commands)?,
            None => 0,
        };
        let loaded = Arc::new(Loaded {
            mc_version: server.mc_version.clone(),
            tree,
            registries,
            vanilla_commands,
            learned_fingerprint: learned.map(|learned| learned.fingerprint),
            learned_added,
        });
        self.servers().entry(id).or_default().loaded = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    fn start_generator(&self, instance: Instance, mc_version: String) -> Result<(), CommandsError> {
        let id = instance.id();
        let mut servers = self.servers();
        let entry = servers.entry(id).or_default();
        if entry.preparing.is_some() {
            return Ok(());
        }
        let spec = JobSpec {
            kind: "commands.prepare",
            title: format!(
                "Reading Minecraft {mc_version}'s commands for {}",
                instance.definition.name
            ),
            instance_id: None,
        };
        let service = self.clone();
        let job_id = self
            .inner
            .jobs
            .spawn(spec, move |job| async move {
                let result = service.generate(&instance, &mc_version, &job).await;
                let mut servers = service.servers();
                let entry = servers.entry(id).or_default();
                entry.preparing = None;
                entry.loaded = None;
                match result {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let reason = error_chain(&error);
                        entry.failed = Some((mc_version, reason.clone()));
                        Err(JobFailure::msg(reason))
                    }
                }
            })
            .map_err(|error| CommandsError::Failed(error.to_string()))?;
        entry.preparing = Some(job_id);
        Ok(())
    }

    async fn generate(
        &self,
        instance: &Instance,
        mc_version: &str,
        job: &gsd_core::jobs::JobContext,
    ) -> Result<(), CommandsError> {
        job.step("Waiting for another server's commands to be read");
        let _turn = self
            .inner
            .generating
            .acquire()
            .await
            .map_err(|_| CommandsError::Cancelled)?;
        let generator = Generator {
            catalog: &self.inner.catalog,
            java: &self.inner.java,
            host: &self.inner.host,
            download_cache: &self.inner.download_cache,
        };
        let vanilla = datagen::generate(generator, instance, mc_version, job).await?;
        write_json(&folder(instance), VANILLA_FILE, &vanilla).await
    }

    /// Asks the running server's `help` in the background; the console's status shows how it went.
    async fn start_learning(&self, instance: Instance, server: MinecraftServer) -> Result<(), CommandsError> {
        let id = instance.id();
        let fingerprint = learn::fingerprint(&instance, &server).await?;
        {
            let mut servers = self.servers();
            let entry = servers.entry(id).or_default();
            if entry.learning {
                return Ok(());
            }
            entry.learning = true;
        }
        let service = self.clone();
        // Subscribed before `help` is sent, so no line of the answer is missed.
        let events = self.inner.events.subscribe();
        tokio::spawn(async move {
            let result = async {
                let lines = learn::ask_help(&service.inner.instances, events, id).await?;
                let commands = help::parse_help(lines.iter().map(String::as_str));
                let learned = LearnedCommands {
                    format: LearnedCommands::FORMAT,
                    fingerprint: fingerprint.clone(),
                    learned_at: Utc::now(),
                    commands,
                };
                write_json(&folder(&instance), LEARNED_FILE, &learned).await
            }
            .await;
            let mut servers = service.servers();
            let entry = servers.entry(id).or_default();
            entry.learning = false;
            entry.loaded = None;
            if let Err(error) = result {
                tracing::warn!(instance = %id, "could not learn the server's commands: {error}");
                entry.learn_failed = Some((fingerprint, error_chain(&error)));
            }
        });
        Ok(())
    }

    fn servers(&self) -> MutexGuard<'_, HashMap<InstanceId, Entry>> {
        self.inner
            .servers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `<instance>/commands`: beside `instance.toml`, out of the server's reach.
fn folder(instance: &Instance) -> PathBuf {
    instance.layout.root().join(FOLDER)
}

async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, CommandsError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > MAX_FILE_BYTES => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CommandsError::Io {
                path: path.to_owned(),
                source,
            });
        }
    }
    let bytes = tokio::fs::read(path).await.map_err(|source| CommandsError::Io {
        path: path.to_owned(),
        source,
    })?;
    // A file from an older format, or a damaged one, is prepared again.
    Ok(serde_json::from_slice(&bytes).ok())
}

/// Written whole or not at all (a temporary file renamed over the old one).
async fn write_json<T: Serialize>(folder: &Path, name: &str, value: &T) -> Result<(), CommandsError> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| CommandsError::Io { path, source }
    };
    tokio::fs::create_dir_all(folder).await.map_err(io(folder))?;
    let bytes = serde_json::to_vec(value).map_err(|error| CommandsError::Failed(error.to_string()))?;
    let temp = folder.join(format!(".{name}.tmp"));
    tokio::fs::write(&temp, bytes).await.map_err(io(&temp))?;
    let path = folder.join(name);
    tokio::fs::rename(&temp, &path).await.map_err(io(&path))
}
