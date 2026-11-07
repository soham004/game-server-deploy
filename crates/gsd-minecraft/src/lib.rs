//! Minecraft: Java Edition adapter.
//!
//! - [`catalog`]: Minecraft versions and server software (Mojang, Paper, Fabric APIs).
//! - [`service`]: creating instances; [`install`]: putting server files in place.
//! - [`adapter`]: the core's `GameAdapter` (console parsing, events, diagnostics).
//!
//! Coming next (see `claude/plan/06-roadmap.md`): Forge/NeoForge/Quilt/Purpur installers, the
//! round-trip `server.properties` editor, RCON, the Server Management Protocol, autocomplete.

pub mod adapter;
pub mod catalog;
pub mod diagnostics;
pub mod install;
pub mod loader;
pub mod log_parser;
pub mod service;
pub mod settings;

pub use adapter::{GAME_ID, MinecraftAdapter};
pub use catalog::{Catalog, CatalogError, Endpoints};
pub use loader::LoaderKind;
pub use service::{CreateError, CreateRequest, MinecraftService};
