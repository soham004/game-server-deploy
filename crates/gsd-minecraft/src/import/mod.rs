//! Importing an existing server folder: folders an admin placed in `<data>/imports/` are
//! recognised (`detect.rs`) and, once the user confirms, copied into a new instance by
//! `service/import.rs`. Only that folder is accepted: browsing arbitrary host paths would let any
//! admin read the whole disk.

mod detect;

use std::path::Path;

pub use detect::{Detected, detect};

/// A regular file (not a link, not a folder).
pub(crate) fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}
