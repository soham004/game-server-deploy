//! Mods and plugins: finding them (Modrinth), installing them with their dependencies, and
//! keeping track of what a server has.
//!
//! - `target.rs`: what fits a server (mods or plugins, loader tags, Minecraft version).
//! - `modrinth/`: the Modrinth API; `model.rs`: what sources describe, independent of the source.
//! - `resolver.rs`: a project plus its required dependencies, at versions that fit.
//! - `index.rs`: the packwiz `.pw.toml` index; `metadata.rs`: what a jar says about itself.
//! - `installed.rs`: what a server has; `install.rs`: carrying out a plan; `service.rs`: the API.
//! - `updates.rs`: newer versions of installed files, naming hand-added files, updating.
//!
//! See `claude/plan/features/mods-and-modpacks.md` and decision D-039.

mod error;
pub mod index;
mod install;
pub mod installed;
pub mod metadata;
pub mod model;
pub mod modrinth;
pub mod resolver;
mod service;
mod target;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod updates;

pub use error::{ContentError, SourceError};
pub use modrinth::Modrinth;
pub use service::{ContentList, ContentService};
pub use target::{ContentKind, Target};
pub use updates::{AvailableUpdate, Identified, UpdateCheck};
