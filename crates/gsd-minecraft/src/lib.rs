//! Minecraft: Java Edition adapter.
//!
//! - [`catalog`]: Minecraft versions and server software (Mojang, Paper, Purpur, Fabric, Quilt,
//!   Forge, `NeoForge`).
//! - [`service`]: creating instances; [`install`]: putting server files in place.
//! - [`adapter`]: the core's `GameAdapter` (console parsing, events, diagnostics).
//! - [`properties`]: the format-preserving `server.properties` editor.
//!
//! - [`logs`]: log history and crash reports; [`control`]: the channel the app controls a server through.
//! - [`players`]: who plays, operators, the allowlist and bans, and changes to them.
//!
//! Coming next (see `claude/plan/06-roadmap.md`): gamerules, worlds, command autocomplete.

pub mod adapter;
pub mod catalog;
pub mod control;
pub mod diagnostics;
pub mod gamerules;
pub mod import;
pub mod install;
pub mod loader;
pub mod log_parser;
pub mod logs;
pub mod nbt;
pub mod players;
pub mod properties;
pub mod service;
pub mod settings;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod worlds;

pub use adapter::{GAME_ID, MinecraftAdapter};
pub use catalog::{Catalog, CatalogError, Endpoints};
pub use loader::LoaderKind;
pub use service::{
    ChangeVersionRequest, CloneRequest, CreateError, CreateRequest, ImportCandidate, ImportRequest,
    MinecraftPaths, MinecraftService, user_jvm_args, with_default_jvm_args,
};
