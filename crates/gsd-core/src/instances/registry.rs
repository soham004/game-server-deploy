//! The set of instances on disk.
//!
//! `instance.toml` files are the source of truth; the registry loads them at startup and keeps
//! them in memory. Creating an instance allocates its slug, game port and sandbox user under
//! one lock, so concurrent creates can never collide.

use std::{
    collections::{BTreeMap, HashSet},
    ops::RangeInclusive,
    path::PathBuf,
    sync::Arc,
};

use chrono::Utc;
use tokio::sync::RwLock;

use crate::{
    events::{EventBus, EventKind},
    ids::InstanceId,
    instances::{
        InstanceDefinition, InstanceLayout, Ports, SCHEMA_VERSION, SandboxIds,
        definition::{BackupPolicy, Behavior, DefinitionError, JavaSettings, LaunchSpec},
        layout::slugify,
    },
    ports::PortRange,
};

/// An instance: its definition and where it lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub definition: InstanceDefinition,
    pub layout: InstanceLayout,
}

impl Instance {
    pub fn id(&self) -> InstanceId {
        self.definition.id
    }
}

/// Everything needed to create an instance except the allocated resources.
#[derive(Debug, Clone)]
pub struct NewInstance {
    pub name: String,
    pub game: String,
    pub server: toml::Table,
    pub launch: LaunchSpec,
    pub java: JavaSettings,
    pub behavior: Behavior,
}

/// Where ports and sandbox users come from.
#[derive(Debug, Clone)]
pub struct AllocationPolicy {
    pub game_ports: PortRange,
    /// Internal ports, never published, for the channel the app controls a server through.
    pub control_ports: PortRange,
    pub sandbox_uids: RangeInclusive<u32>,
    /// Skip ports another program is listening on (disabled in tests).
    pub probe_ports: bool,
}

impl AllocationPolicy {
    pub const DEFAULT_SANDBOX_UIDS: RangeInclusive<u32> = 20000..=29999;
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("no free game port left in {0}")]
    NoFreePort(PortRange),
    #[error("no free sandbox user id left")]
    NoFreeUid,
    #[error("instance not found")]
    NotFound,
    #[error("filesystem error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Definition(#[from] DefinitionError),
}

/// An `instance.toml` that couldn't be loaded at startup (reported, not fatal).
#[derive(Debug)]
pub struct LoadProblem {
    pub path: PathBuf,
    pub error: DefinitionError,
}

#[derive(Debug, Clone)]
pub struct InstanceRegistry {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    instances_dir: PathBuf,
    trash_dir: PathBuf,
    policy: AllocationPolicy,
    events: EventBus,
    entries: RwLock<BTreeMap<InstanceId, Instance>>,
}

impl InstanceRegistry {
    /// Loads every `<instances_dir>/*/instance.toml`. Broken ones are returned as problems.
    pub fn load(
        instances_dir: PathBuf,
        trash_dir: PathBuf,
        policy: AllocationPolicy,
        events: EventBus,
    ) -> Result<(Self, Vec<LoadProblem>), RegistryError> {
        std::fs::create_dir_all(&instances_dir).map_err(|source| RegistryError::Io {
            path: instances_dir.clone(),
            source,
        })?;
        let mut entries = BTreeMap::new();
        let mut problems = Vec::new();

        let dir_entries = std::fs::read_dir(&instances_dir).map_err(|source| RegistryError::Io {
            path: instances_dir.clone(),
            source,
        })?;
        for entry in dir_entries.filter_map(Result::ok) {
            let layout = InstanceLayout::new(entry.path());
            let path = layout.definition_path();
            if !path.is_file() {
                continue;
            }
            match InstanceDefinition::load(&path) {
                Ok(definition) => {
                    entries.insert(definition.id, Instance { definition, layout });
                }
                Err(error) => problems.push(LoadProblem { path, error }),
            }
        }

        let registry = Self {
            inner: Arc::new(Inner {
                instances_dir,
                trash_dir,
                policy,
                events,
                entries: RwLock::new(entries),
            }),
        };
        Ok((registry, problems))
    }

    /// All instances, sorted by name.
    pub async fn list(&self) -> Vec<Instance> {
        let mut instances: Vec<Instance> = self.inner.entries.read().await.values().cloned().collect();
        instances.sort_by_key(|instance| instance.definition.name.to_lowercase());
        instances
    }

    pub async fn get(&self, id: InstanceId) -> Option<Instance> {
        self.inner.entries.read().await.get(&id).cloned()
    }

    /// Allocates a slug, port and sandbox user, creates the directories and writes `instance.toml`.
    pub async fn create(&self, new: NewInstance) -> Result<Instance, RegistryError> {
        let mut entries = self.inner.entries.write().await;

        let used_ports: HashSet<u16> = entries.values().map(|i| i.definition.ports.game).collect();
        let used_uids: HashSet<u32> = entries.values().map(|i| i.definition.sandbox.uid).collect();
        let game_port = pick_port(
            self.inner.policy.game_ports,
            &used_ports,
            self.inner.policy.probe_ports,
        )
        .ok_or(RegistryError::NoFreePort(self.inner.policy.game_ports))?;
        let uid = self
            .inner
            .policy
            .sandbox_uids
            .clone()
            .find(|uid| !used_uids.contains(uid))
            .ok_or(RegistryError::NoFreeUid)?;
        let ids = SandboxIds { uid, gid: uid };

        let layout = InstanceLayout::new(self.unique_root(&new.name));
        let io_error = |source| RegistryError::Io {
            path: layout.root().to_owned(),
            source,
        };
        layout.create(ids).map_err(io_error)?;

        let definition = InstanceDefinition {
            schema: SCHEMA_VERSION,
            id: InstanceId::generate(),
            name: new.name.trim().to_owned(),
            game: new.game,
            created_at: Utc::now(),
            installed_at: None,
            server: new.server,
            launch: new.launch,
            java: new.java,
            behavior: new.behavior,
            backups: BackupPolicy::default(),
            ports: Ports {
                game: game_port,
                control: None,
            },
            sandbox: ids,
            modpack: None,
        };
        if let Err(error) = definition.save(&layout.definition_path()) {
            let _ = std::fs::remove_dir_all(layout.root());
            return Err(error.into());
        }

        let instance = Instance { definition, layout };
        entries.insert(instance.id(), instance.clone());
        drop(entries);
        self.inner
            .events
            .publish(Some(instance.id()), EventKind::InstanceChanged);
        Ok(instance)
    }

    /// Gives the server its control port if it has none. With the range used up, the server still runs,
    /// only without the control channel, so that is a warning and not a failure to start.
    pub async fn ensure_control_port(&self, id: InstanceId) -> Result<Instance, RegistryError> {
        let mut entries = self.inner.entries.write().await;
        let instance = entries.get(&id).ok_or(RegistryError::NotFound)?;
        if instance.definition.ports.control.is_some() {
            return Ok(instance.clone());
        }
        let used: HashSet<u16> = entries
            .values()
            .flat_map(|other| [Some(other.definition.ports.game), other.definition.ports.control])
            .flatten()
            .collect();
        let range = self.inner.policy.control_ports;
        let Some(port) = pick_port(range, &used, self.inner.policy.probe_ports) else {
            tracing::warn!(%range, "no free control port left; this server runs without a control channel");
            return Ok(instance.clone());
        };
        let instance = entries.get_mut(&id).ok_or(RegistryError::NotFound)?;
        let mut updated = instance.definition.clone();
        updated.ports.control = Some(port);
        updated.save(&instance.layout.definition_path())?;
        instance.definition = updated;
        Ok(instance.clone())
    }

    /// Applies `change` to the definition and saves it.
    pub async fn update(
        &self,
        id: InstanceId,
        change: impl FnOnce(&mut InstanceDefinition),
    ) -> Result<Instance, RegistryError> {
        let mut entries = self.inner.entries.write().await;
        let instance = entries.get_mut(&id).ok_or(RegistryError::NotFound)?;
        let mut updated = instance.definition.clone();
        change(&mut updated);
        updated.id = id; // the id is immutable
        updated.save(&instance.layout.definition_path())?;
        instance.definition = updated;
        let instance = instance.clone();
        drop(entries);
        self.inner.events.publish(Some(id), EventKind::InstanceChanged);
        Ok(instance)
    }

    /// Moves the instance folder to the trash (recoverable) and forgets it.
    pub async fn remove(&self, id: InstanceId) -> Result<(), RegistryError> {
        let mut entries = self.inner.entries.write().await;
        let instance = entries.get(&id).ok_or(RegistryError::NotFound)?;
        let trash = &self.inner.trash_dir;
        let io_error = |path: PathBuf| move |source| RegistryError::Io { path, source };
        std::fs::create_dir_all(trash).map_err(io_error(trash.clone()))?;
        let destination = trash.join(crate::trash::entry_name(&instance.layout.slug(), Utc::now()));
        std::fs::rename(instance.layout.root(), &destination).map_err(io_error(destination.clone()))?;
        entries.remove(&id);
        drop(entries);
        self.inner.events.publish(Some(id), EventKind::InstanceDeleted);
        Ok(())
    }

    /// `<instances>/<slug>`, adding `-2`, `-3`, … when the slug is taken on disk.
    fn unique_root(&self, name: &str) -> PathBuf {
        let base = slugify(name);
        let mut candidate = self.inner.instances_dir.join(&base);
        let mut n = 2;
        while candidate.exists() {
            candidate = self.inner.instances_dir.join(format!("{base}-{n}"));
            n += 1;
        }
        candidate
    }
}

/// The first port in `range` that no instance uses and (optionally) nothing is listening on.
fn pick_port(range: PortRange, used: &HashSet<u16>, probe: bool) -> Option<u16> {
    range
        .ports()
        .find(|port| !used.contains(port) && (!probe || is_port_free(*port)))
}

fn is_port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::definition::{JvmPreset, RuntimeChoice};

    fn policy(ports: &str) -> AllocationPolicy {
        AllocationPolicy {
            game_ports: ports.parse().unwrap(),
            control_ports: "35565-35566".parse().unwrap(),
            sandbox_uids: 20000..=20001,
            probe_ports: false,
        }
    }

    fn new_instance(name: &str) -> NewInstance {
        NewInstance {
            name: name.into(),
            game: "minecraft".into(),
            server: toml::Table::new(),
            launch: LaunchSpec::Jar {
                jar: "server.jar".into(),
                args: vec!["nogui".into()],
            },
            java: JavaSettings {
                major: 21,
                runtime: RuntimeChoice::Auto,
                min_memory_mb: 512,
                max_memory_mb: 1024,
                preset: JvmPreset::None,
                extra_jvm_args: vec![],
            },
            behavior: Behavior::default(),
        }
    }

    fn registry(dir: &std::path::Path, ports: &str) -> InstanceRegistry {
        InstanceRegistry::load(
            dir.join("instances"),
            dir.join("trash"),
            policy(ports),
            EventBus::new(),
        )
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn create_allocates_unique_slugs_ports_and_users() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path(), "25565-25566");

        let first = registry.create(new_instance("Survival")).await.unwrap();
        let second = registry.create(new_instance("Survival")).await.unwrap();
        assert_eq!(first.layout.slug(), "survival");
        assert_eq!(second.layout.slug(), "survival-2");
        assert_eq!(
            (first.definition.ports.game, second.definition.ports.game),
            (25565, 25566)
        );
        assert_ne!(first.definition.sandbox.uid, second.definition.sandbox.uid);
        assert!(first.layout.definition_path().is_file());

        assert!(matches!(
            registry.create(new_instance("Third")).await,
            Err(RegistryError::NoFreePort(_))
        ));
    }

    #[tokio::test]
    async fn reloads_from_disk_and_reports_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let created = registry(dir.path(), "25565-25570")
            .create(new_instance("Creative"))
            .await
            .unwrap();
        std::fs::create_dir_all(dir.path().join("instances/broken")).unwrap();
        std::fs::write(dir.path().join("instances/broken/instance.toml"), "not = [valid").unwrap();

        let (reloaded, problems) = InstanceRegistry::load(
            dir.path().join("instances"),
            dir.path().join("trash"),
            policy("25565-25570"),
            EventBus::new(),
        )
        .unwrap();
        assert_eq!(
            reloaded.get(created.id()).await.unwrap().definition,
            created.definition
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].path.ends_with("broken/instance.toml"));
    }

    #[tokio::test]
    async fn update_persists_and_keeps_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path(), "25565-25570");
        let instance = registry.create(new_instance("Old name")).await.unwrap();

        let updated = registry
            .update(instance.id(), |definition| {
                definition.name = "New name".into();
                definition.id = InstanceId::generate();
            })
            .await
            .unwrap();
        assert_eq!(updated.id(), instance.id());
        let on_disk = InstanceDefinition::load(&instance.layout.definition_path()).unwrap();
        assert_eq!(on_disk.name, "New name");
    }

    #[tokio::test]
    async fn remove_moves_the_folder_to_trash() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path(), "25565-25570");
        let instance = registry.create(new_instance("Doomed")).await.unwrap();
        std::fs::write(instance.layout.server_dir().join("world.dat"), "precious").unwrap();

        registry.remove(instance.id()).await.unwrap();
        assert!(registry.get(instance.id()).await.is_none());
        assert!(!instance.layout.root().exists());
        let trashed: Vec<_> = std::fs::read_dir(dir.path().join("trash")).unwrap().collect();
        assert_eq!(trashed.len(), 1);
        assert!(matches!(
            registry.remove(instance.id()).await,
            Err(RegistryError::NotFound)
        ));
    }

    /// A port held here is skipped. The port after it is only known to be free at one moment:
    /// tests running alongside open connections from ports the system hands out, and one can
    /// take it meanwhile (seen in CI). So a pair is tried a few times, and the probe may only
    /// pass over that port when it really is taken by then.
    #[tokio::test]
    async fn port_probe_skips_ports_in_use() {
        for _ in 0..20 {
            let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
            let busy = listener.local_addr().unwrap().port();
            let Some(next) = busy.checked_add(1).filter(|next| is_port_free(*next)) else {
                continue;
            };
            let range = PortRange::new(busy, next).unwrap();
            assert_eq!(pick_port(range, &HashSet::new(), false), Some(busy), "no probe");
            match pick_port(range, &HashSet::new(), true) {
                Some(port) => {
                    assert_eq!(port, next, "the held port is skipped");
                    return;
                }
                None => assert!(!is_port_free(next), "{next} was free, yet skipped"),
            }
        }
        panic!("no free port next to a held one in 20 tries");
    }

    #[tokio::test]
    async fn creation_publishes_an_event() {
        let dir = tempfile::tempdir().unwrap();
        let events = EventBus::new();
        let mut receiver = events.subscribe();
        let (registry, _) = InstanceRegistry::load(
            dir.path().join("i"),
            dir.path().join("t"),
            policy("25565-25570"),
            events,
        )
        .unwrap();
        let instance = registry.create(new_instance("Evented")).await.unwrap();
        let event = receiver.recv().await.unwrap();
        assert_eq!(event.instance_id, Some(instance.id()));
    }
}
