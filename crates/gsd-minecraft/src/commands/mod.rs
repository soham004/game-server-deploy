//! Console command autocomplete.
//!
//! Three layers of what a server's commands are:
//! 1. **Vanilla, exact:** the command tree and ids of the server's Minecraft version, from the game's
//!    own data generator (`datagen.rs`), run once per server and version.
//! 2. **Mods and plugins, approximate:** the commands its `help` lists (`help.rs`, `learn.rs`), asked
//!    once per set of mods or plugins. Their names, and their first words where `help` says.
//! 3. **What's live:** players online.
//!
//! [`complete::complete`] puts them together for the text being typed; [`service::CommandService`]
//! keeps them per server (in its folder, `commands/`, which only the app can write).

pub mod complete;
pub mod datagen;
pub mod help;
pub mod learn;
pub mod registries;
pub mod service;
pub mod tree;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use complete::{Completion, Signature, SignaturePart, Suggestion, SuggestionKind};
pub use help::LearnedCommand;
pub use registries::Registries;
pub use service::{CommandService, CommandsStatus, LearnedState, VanillaState};
pub use tree::{CommandTree, Node, NodeKind};

/// The data generator's output for one Minecraft version, as saved (`commands/vanilla.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VanillaCommands {
    pub format: u32,
    pub mc_version: String,
    pub generated_at: DateTime<Utc>,
    pub tree: CommandTree,
    pub registries: Registries,
}

impl VanillaCommands {
    pub const FORMAT: u32 = 1;
}

/// What a server's `help` said, as saved (`commands/learned.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedCommands {
    pub format: u32,
    /// The mods and plugins it was asked with (see [`learn::fingerprint`]).
    pub fingerprint: String,
    pub learned_at: DateTime<Utc>,
    pub commands: Vec<LearnedCommand>,
}

impl LearnedCommands {
    pub const FORMAT: u32 = 1;
}

#[derive(Debug, thiserror::Error)]
pub enum CommandsError {
    #[error(transparent)]
    Catalog(#[from] crate::catalog::CatalogError),
    #[error(transparent)]
    Install(#[from] crate::install::InstallError),
    #[error(transparent)]
    Java(#[from] gsd_core::java::JavaError),
    #[error(transparent)]
    Sandbox(#[from] gsd_core::supervisor::SandboxUnusable),
    #[error(transparent)]
    Files(#[from] gsd_core::files::FilesError),
    #[error(transparent)]
    Path(#[from] gsd_core::files::PathError),
    #[error(transparent)]
    Random(#[from] gsd_core::random::RandomError),
    #[error(transparent)]
    Tree(#[from] tree::TreeError),
    #[error("the server jar can't be read: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Instance(#[from] gsd_core::instances::InstanceError),
    #[error(transparent)]
    Settings(#[from] crate::settings::SettingsError),
    #[error("filesystem error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Failed(String),
    /// Nothing to generate for this version (it predates the command list).
    #[error("{0}")]
    Unavailable(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<std::io::Error> for CommandsError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

#[cfg(test)]
mod tests;
