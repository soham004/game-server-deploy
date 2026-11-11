//! Installing server files into an instance's server directory.
//!
//! `pipeline.rs` runs an install: `plan.rs` turns the loader into steps (download a server jar,
//! or download and run the loader's installer), `execute.rs` runs them (installers sandboxed like
//! the server), `run_script.rs` finds how Forge/`NeoForge` servers start, and `server_files.rs`
//! writes `eula.txt` and the initial `server.properties`.
//!
//! An installer is downloaded code, and on a reinstall or a version change the folder has been a
//! running server's. So everything the app itself reads or writes in it goes through the file
//! manager (`gsd_core::files::ServerFiles`): no link in the folder can lead it elsewhere.

mod execute;
mod pipeline;
mod plan;
mod run_script;
mod server_files;

use std::path::PathBuf;

pub(crate) use execute::cached_download;
pub use pipeline::{InstallTarget, finish_existing, install};
pub use plan::{InstallPlan, InstallStep, LaunchPlan, SERVER_JAR};
pub use server_files::{point_at_port, point_doc_at_port};

use crate::catalog::CatalogError;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Download(gsd_core::net::DownloadError),
    #[error(transparent)]
    Sandbox(#[from] gsd_core::supervisor::SandboxUnusable),
    #[error("the installer could not be started")]
    InstallerDidNotStart {
        #[source]
        source: std::io::Error,
    },
    #[error("the installer failed (exit code {}); its output is in the job log", code.map_or_else(|| "none".into(), |c| c.to_string()))]
    InstallerFailed { code: Option<i32> },
    #[error("the installer did not finish within {minutes} minutes")]
    InstallerTimedOut { minutes: u64 },
    #[error("the installer finished, but the server can't be started: {detail}")]
    LaunchNotFound { detail: String },
    #[error("cancelled")]
    Cancelled,
    /// Something in the server folder isn't what an install can work with (the text says what).
    #[error(transparent)]
    Files(#[from] gsd_core::files::FilesError),
    #[error("filesystem error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
