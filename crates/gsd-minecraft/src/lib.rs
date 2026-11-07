//! Minecraft: Java Edition adapter.
//!
//! - [`catalog`]: Minecraft versions and server software (Mojang, Paper, Purpur, Fabric, Quilt,
//!   Forge, `NeoForge`).
//! - [`service`]: creating instances; [`install`]: putting server files in place.
//! - [`adapter`]: the core's `GameAdapter` (console parsing, events, diagnostics).
//! - [`properties`]: the format-preserving `server.properties` editor.
//!
//! Coming next (see `claude/plan/06-roadmap.md`): RCON, the Server Management Protocol,
//! command autocomplete.

pub mod adapter;
pub mod catalog;
pub mod diagnostics;
pub mod import;
pub mod install;
pub mod loader;
pub mod log_parser;
pub mod properties;
pub mod service;
pub mod settings;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use adapter::{GAME_ID, MinecraftAdapter};
pub use catalog::{Catalog, CatalogError, Endpoints};
pub use loader::LoaderKind;
pub use service::{
    CloneRequest, CreateError, CreateRequest, ImportCandidate, ImportRequest, MinecraftPaths,
    MinecraftService, user_jvm_args, with_default_jvm_args,
};
