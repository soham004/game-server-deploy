//! Browsing and changing an instance's server files (the file manager).
//!
//! `path.rs` checks user paths lexically; `root.rs` does every operation through a `cap-std`
//! directory handle so nothing can escape the server folder, not even through symlinks the game
//! server plants (it runs as another user and may be compromised); `regular.rs` opens files to read
//! without being held up by a FIFO or an endless file put in their place; `archive.rs` extracts and
//! builds archives; `service.rs` is the async API. Files the app creates are handed to the
//! instance's user; deleted files go to the instance's `trash/`, outside the server's reach.

mod archive;
pub mod check;
mod copy;
mod error;
mod path;
pub(crate) mod regular;
mod root;
mod service;

pub use archive::{ExtractLimits, ExtractSummary, unpack_zip};
pub use copy::{CopySummary, copy_server_dir};
pub use error::FilesError;
pub use path::{PathError, SafeRelPath};
pub use regular::read_small_file;
pub use root::{Entry, EntryKind, MAX_TEXT_BYTES, TextFile, revision};
pub use service::ServerFiles;

#[cfg(test)]
mod tests;
