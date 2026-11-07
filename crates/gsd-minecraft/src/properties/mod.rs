//! `server.properties`: a format-preserving parser (`format.rs`), what each key means
//! (`schema.rs` + `server_properties.json`), and reading/changing an instance's file safely
//! (`editor.rs`).

mod editor;
mod format;
pub mod schema;

pub use editor::{
    FieldError, KeyValue, MAX_FILE_BYTES, PropertiesEdit, PropertiesError, PropertiesSnapshot, apply, read,
};
pub use format::PropertiesDoc;
pub use schema::{Category, PropertySchema, ValueKind};
