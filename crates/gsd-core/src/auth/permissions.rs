//! The RBAC model.
//!
//! - [`GlobalRole`]: `owner`, `admin` or `member`. Owners and admins can access every instance.
//! - [`Permission`]: something a user may do *on one instance*.
//! - [`Grant`]: what a `member` may do on a specific instance (a preset or a custom set).
//! - [`GlobalPermission`]: app-wide abilities (create instances, manage users, …).
//!
//! The matrix is documented in `claude/plan/features/security-and-auth.md`; keep both in sync.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An action on a single instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// See the instance, its status and metrics.
    #[serde(rename = "instance.view")]
    InstanceView,
    #[serde(rename = "console.read")]
    ConsoleRead,
    #[serde(rename = "logs.read")]
    LogsRead,
    /// Kick, ban, whitelist, op up to level 2.
    #[serde(rename = "players.manage")]
    PlayersManage,
    /// Send any console command.
    #[serde(rename = "console.write")]
    ConsoleWrite,
    /// Start, stop, restart, kill.
    #[serde(rename = "lifecycle")]
    Lifecycle,
    #[serde(rename = "backups.create")]
    BackupsCreate,
    #[serde(rename = "schedules.manage")]
    SchedulesManage,
    #[serde(rename = "files.read")]
    FilesRead,
    #[serde(rename = "files.write")]
    FilesWrite,
    /// Edit `server.properties` and gamerules.
    #[serde(rename = "properties.write")]
    PropertiesWrite,
    /// Install, update and remove mods, plugins and packs.
    #[serde(rename = "content.manage")]
    ContentManage,
    /// Restore or delete backups.
    #[serde(rename = "backups.restore")]
    BackupsRestore,
    /// Memory, JVM arguments, Java (from the managed list), auto-start.
    #[serde(rename = "instance.settings")]
    InstanceSettings,
    /// Update the server software or change the Minecraft version.
    #[serde(rename = "instance.update")]
    InstanceUpdate,
}

impl Permission {
    pub const ALL: [Self; 15] = [
        Self::InstanceView,
        Self::ConsoleRead,
        Self::LogsRead,
        Self::PlayersManage,
        Self::ConsoleWrite,
        Self::Lifecycle,
        Self::BackupsCreate,
        Self::SchedulesManage,
        Self::FilesRead,
        Self::FilesWrite,
        Self::PropertiesWrite,
        Self::ContentManage,
        Self::BackupsRestore,
        Self::InstanceSettings,
        Self::InstanceUpdate,
    ];

    const fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// A set of instance permissions (a compact bit set).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PermissionSet(u32);

impl PermissionSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn all() -> Self {
        Permission::ALL.into_iter().collect()
    }

    #[must_use]
    pub const fn with(self, permission: Permission) -> Self {
        Self(self.0 | permission.bit())
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, permission: Permission) -> bool {
        self.0 & permission.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Permission> {
        Permission::ALL
            .into_iter()
            .filter(move |permission| self.contains(*permission))
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        iter.into_iter().fold(Self::empty(), Self::with)
    }
}

impl fmt::Debug for PermissionSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl Serialize for PermissionSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for PermissionSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<Permission>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

const VIEWER: PermissionSet = PermissionSet::empty()
    .with(Permission::InstanceView)
    .with(Permission::ConsoleRead)
    .with(Permission::LogsRead);
const MODERATOR: PermissionSet = VIEWER.with(Permission::PlayersManage);
const OPERATOR: PermissionSet = MODERATOR
    .with(Permission::ConsoleWrite)
    .with(Permission::Lifecycle)
    .with(Permission::BackupsCreate);

/// What a `member` may do on one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Viewer,
    Moderator,
    Operator,
    Manager,
    Custom(PermissionSet),
}

impl Grant {
    pub fn permissions(self) -> PermissionSet {
        match self {
            Self::Viewer => VIEWER,
            Self::Moderator => MODERATOR,
            Self::Operator => OPERATOR,
            Self::Manager => PermissionSet::all(),
            Self::Custom(set) => set,
        }
    }

    /// The value stored in the `instance_grants.preset` column.
    pub fn preset_name(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Moderator => "moderator",
            Self::Operator => "operator",
            Self::Manager => "manager",
            Self::Custom(_) => "custom",
        }
    }

    /// Rebuilds a grant from its stored form; `None` if the preset name is unknown or a custom
    /// grant has no permission list.
    pub fn from_stored(preset: &str, custom: Option<PermissionSet>) -> Option<Self> {
        match (preset, custom) {
            ("viewer", _) => Some(Self::Viewer),
            ("moderator", _) => Some(Self::Moderator),
            ("operator", _) => Some(Self::Operator),
            ("manager", _) => Some(Self::Manager),
            ("custom", Some(set)) => Some(Self::Custom(set)),
            _ => None,
        }
    }
}

/// An app-wide ability that is not tied to one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GlobalPermission {
    #[serde(rename = "instances.create")]
    InstanceCreate,
    #[serde(rename = "instances.delete")]
    InstanceDelete,
    #[serde(rename = "users.manage")]
    UsersManage,
    #[serde(rename = "settings.manage")]
    SettingsManage,
    /// Point an instance at a Java binary outside the managed runtimes.
    #[serde(rename = "runtimes.custom")]
    CustomRuntime,
    /// Loosen an instance's sandbox (e.g. allow extra outbound ports).
    #[serde(rename = "sandbox.exceptions")]
    SandboxExceptions,
}

/// The app-wide role of a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalRole {
    /// Everything, including users and app settings.
    Owner,
    /// Every instance and instance lifecycle; no user or app-setting management.
    Admin,
    /// Only what per-instance grants allow; cannot create instances (D-023).
    Member,
}

impl GlobalRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub fn global_permissions(self) -> &'static [GlobalPermission] {
        use GlobalPermission::{
            CustomRuntime, InstanceCreate, InstanceDelete, SandboxExceptions, SettingsManage, UsersManage,
        };
        match self {
            Self::Owner => &[
                InstanceCreate,
                InstanceDelete,
                UsersManage,
                SettingsManage,
                CustomRuntime,
                SandboxExceptions,
            ],
            Self::Admin => &[InstanceCreate, InstanceDelete, CustomRuntime, SandboxExceptions],
            Self::Member => &[],
        }
    }

    /// Owners and admins hold every instance permission on every instance.
    pub fn can_access_all_instances(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// An unknown role name was found (only possible with a corrupted database).
#[derive(Debug, thiserror::Error)]
#[error("unknown role {0:?}")]
pub struct UnknownRole(String);

impl FromStr for GlobalRole {
    type Err = UnknownRole;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            other => Err(UnknownRole(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_cumulative() {
        let presets = [Grant::Viewer, Grant::Moderator, Grant::Operator, Grant::Manager];
        for pair in presets.windows(2) {
            let (lower, higher) = (pair[0].permissions(), pair[1].permissions());
            assert!(lower.iter().all(|p| higher.contains(p)), "{pair:?}");
        }
    }

    #[test]
    fn operator_can_run_commands_but_not_touch_files() {
        let operator = Grant::Operator.permissions();
        assert!(operator.contains(Permission::ConsoleWrite));
        assert!(operator.contains(Permission::Lifecycle));
        assert!(!operator.contains(Permission::FilesWrite));
        assert!(!operator.contains(Permission::ContentManage));
    }

    #[test]
    fn permission_sets_serialize_as_names() {
        let set = PermissionSet::empty()
            .with(Permission::FilesRead)
            .with(Permission::Lifecycle);
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(json, r#"["lifecycle","files.read"]"#);
        let back: PermissionSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn only_owners_manage_users_and_members_cannot_create() {
        assert!(
            GlobalRole::Owner
                .global_permissions()
                .contains(&GlobalPermission::UsersManage)
        );
        assert!(
            !GlobalRole::Admin
                .global_permissions()
                .contains(&GlobalPermission::UsersManage)
        );
        assert_eq!(GlobalRole::Member.global_permissions(), []);
    }

    #[test]
    fn roles_round_trip() {
        for role in [GlobalRole::Owner, GlobalRole::Admin, GlobalRole::Member] {
            assert_eq!(role.as_str().parse::<GlobalRole>().unwrap(), role);
        }
    }
}
