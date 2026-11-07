//! What can go wrong with a file operation, in terms the user can act on.

use std::io;

use super::{PathError, SafeRelPath};

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("{0} does not exist")]
    NotFound(SafeRelPath),
    #[error("{0} already exists")]
    AlreadyExists(SafeRelPath),
    #[error("{0} is not a folder")]
    NotADirectory(SafeRelPath),
    #[error("{0} is a folder")]
    IsADirectory(SafeRelPath),
    #[error("{0} is not a text file; download it instead")]
    NotText(SafeRelPath),
    #[error("{path} is larger than {limit} bytes")]
    TooLarge { path: SafeRelPath, limit: u64 },
    #[error("{0} changed since it was opened; reload it and apply your changes again")]
    Conflict(SafeRelPath),
    #[error("{0}")]
    InvalidOperation(String),
    /// `cap-std` refused to leave the server folder (e.g. through a symlink), or the OS said no.
    #[error("{0} is outside the server folder or not accessible")]
    Forbidden(SafeRelPath),
    #[error("{0} is not a valid archive: {1}")]
    BadArchive(SafeRelPath, String),
    #[error("file operation failed on {path}")]
    Io {
        path: SafeRelPath,
        #[source]
        source: io::Error,
    },
}

impl FilesError {
    /// Maps an I/O error on `path` to the most specific variant.
    pub(crate) fn io(path: &SafeRelPath, source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path.clone()),
            io::ErrorKind::AlreadyExists => Self::AlreadyExists(path.clone()),
            io::ErrorKind::NotADirectory => Self::NotADirectory(path.clone()),
            io::ErrorKind::IsADirectory => Self::IsADirectory(path.clone()),
            io::ErrorKind::PermissionDenied => Self::Forbidden(path.clone()),
            _ => Self::Io {
                path: path.clone(),
                source,
            },
        }
    }
}
