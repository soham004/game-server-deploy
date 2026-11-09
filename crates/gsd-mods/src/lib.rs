//! Mods and plugins: finding them (Modrinth, Hangar, `CurseForge`), installing them with their
//! dependencies, and keeping track of what a server has.
//!
//! - `target.rs`: what fits a server (mods or plugins, loader tags, Minecraft version).
//! - `modrinth/`, `hangar/`, `curseforge/`: the sources' APIs; `source.rs`: one interface over them;
//!   `model.rs`: what sources describe, independent of the source; `fingerprint.rs`: `CurseForge`'s
//!   file hash.
//! - `resolver.rs`: projects plus their required dependencies, at versions that fit.
//! - `index.rs`: the packwiz `.pw.toml` index; `metadata.rs`: what a jar says about itself.
//! - `installed.rs`: what a server has; `install.rs`: carrying out a plan; `service.rs`: the API.
//! - `updates.rs`: newer versions of installed files, naming hand-added files, updating.
//! - `modpack/`: an uploaded pack or server zip becoming a server.
//!
//! See `claude/plan/features/mods-and-modpacks.md` and decisions D-039, D-043, D-044, D-045.

pub mod curseforge;
mod error;
pub mod fingerprint;
pub mod hangar;
mod http;
pub mod index;
mod install;
pub mod installed;
pub mod metadata;
pub mod model;
pub mod modpack;
pub mod modrinth;
pub mod resolver;
mod service;
mod source;
mod target;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod updates;

pub use error::{ContentError, SourceError};
pub use hangar::Hangar;
pub use modrinth::Modrinth;
pub use service::{ContentList, ContentService};
pub use source::{SourceApis, Sources};
pub use target::{ContentKind, Target};
pub use updates::{AvailableUpdate, Identified, UpdateCheck};
