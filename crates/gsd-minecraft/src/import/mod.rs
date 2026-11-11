//! Importing an existing server folder: folders an admin placed in `<data>/imports/` are
//! recognised (`detect.rs`) and, once the user confirms, copied into a new instance by
//! `service/import.rs`. Only that folder is accepted: browsing arbitrary host paths would let any
//! admin read the whole disk.

mod detect;

pub use detect::{Detected, detect};
