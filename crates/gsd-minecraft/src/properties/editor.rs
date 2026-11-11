//! Reading and changing an instance's `server.properties`.
//!
//! Every read carries a `revision` (hash of the file). An edit must send back the revision it
//! started from; if the file changed in the meantime (the server or someone else wrote it), the
//! edit is refused instead of silently overwriting. Edits are validated against the schema, may
//! not change app-managed keys (the ports the sandbox allows), and are written atomically and
//! handed to the instance's user.
//!
//! The file is the server's own: it is read and written through the file manager
//! ([`ServerFiles`]), which doesn't follow a link out of the server folder and reads no more than
//! a settings file can hold, whatever the server put there.

use std::collections::BTreeMap;

use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    instances::Instance,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{PropertiesDoc, schema};

const FILE: &str = "server.properties";
/// Larger than any real `server.properties`; protects the server from a pasted novel.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// Serialises read-check-write sequences so two edits can't interleave (edits are rare).
static WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The file as the editor sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PropertiesSnapshot {
    /// Send this back with an edit.
    pub revision: String,
    /// False before the server first started and nothing was written yet.
    pub exists: bool,
    pub text: String,
    /// Every key in the file with its effective value, in file order.
    pub values: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// A change to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertiesEdit {
    /// Key → new value, or `None` to remove the key.
    Changes(BTreeMap<String, Option<String>>),
    /// The whole file, as typed in the raw editor.
    Replace(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub key: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PropertiesError {
    #[error("server.properties changed since it was loaded; reload and apply your changes again")]
    Conflict,
    #[error("some values are invalid")]
    Invalid(Vec<FieldError>),
    #[error("the file is larger than {MAX_FILE_BYTES} bytes")]
    TooLarge,
    #[error(transparent)]
    Files(FilesError),
}

impl From<FilesError> for PropertiesError {
    fn from(error: FilesError) -> Self {
        match error {
            FilesError::TooLarge { .. } => Self::TooLarge,
            // The server rewrote the file between the look and the save.
            FilesError::Conflict(_) | FilesError::AlreadyExists(_) => Self::Conflict,
            other => Self::Files(other),
        }
    }
}

/// Reads the current file (an empty document if it doesn't exist yet).
pub async fn read(instance: &Instance) -> Result<PropertiesSnapshot, PropertiesError> {
    let files = ServerFiles::open(instance).await?;
    let current = read_bytes(&files).await?;
    Ok(snapshot(
        current.as_deref().unwrap_or_default(),
        current.is_some(),
    ))
}

/// Applies `edit` if the file is still at `expected_revision`; returns the new state.
pub async fn apply(
    instance: &Instance,
    edit: &PropertiesEdit,
    expected_revision: &str,
) -> Result<PropertiesSnapshot, PropertiesError> {
    let _guard = WRITE_LOCK.lock().await;
    let files = ServerFiles::open(instance).await?;
    let on_disk = read_bytes(&files).await?;
    let bytes = on_disk.as_deref().unwrap_or_default();
    if revision(bytes) != expected_revision {
        return Err(PropertiesError::Conflict);
    }
    let current = PropertiesDoc::from_bytes(bytes);
    let updated = edited(&current, edit)?;
    check(&current, &updated)?;

    let text = updated.render();
    if text.len() > MAX_FILE_BYTES {
        return Err(PropertiesError::TooLarge);
    }
    // Saved only over the file just read (or over none): the file manager checks again.
    let replaces = on_disk.is_some().then(|| expected_revision.to_owned());
    files
        .write_bytes(file()?, text.clone().into_bytes(), replaces)
        .await?;
    Ok(snapshot(text.as_bytes(), true))
}

impl PropertiesSnapshot {
    /// Without the values of sensitive keys (for users who may not edit properties).
    #[must_use]
    pub fn redacted(&self) -> Self {
        let mut doc = PropertiesDoc::parse(&self.text);
        let mut values = self.values.clone();
        for entry in &mut values {
            if schema::find(&entry.key).is_some_and(|s| s.sensitive) && !entry.value.is_empty() {
                entry.value = String::new();
                doc.set(&entry.key, "");
            }
        }
        Self {
            revision: self.revision.clone(),
            exists: self.exists,
            text: doc.render(),
            values,
        }
    }
}

fn edited(current: &PropertiesDoc, edit: &PropertiesEdit) -> Result<PropertiesDoc, PropertiesError> {
    match edit {
        PropertiesEdit::Replace(text) => {
            if text.len() > MAX_FILE_BYTES {
                return Err(PropertiesError::TooLarge);
            }
            Ok(PropertiesDoc::parse(text))
        }
        PropertiesEdit::Changes(changes) => {
            let mut doc = current.clone();
            for (key, value) in changes {
                if key.trim().is_empty() {
                    return Err(invalid(key, "keys can't be empty"));
                }
                match value {
                    Some(value) => doc.set(key, value),
                    None => doc.remove(key),
                }
            }
            Ok(doc)
        }
    }
}

/// Known keys must hold valid values, and managed keys must keep their current value.
fn check(current: &PropertiesDoc, updated: &PropertiesDoc) -> Result<(), PropertiesError> {
    let mut errors = Vec::new();
    for entry in schema::all() {
        let before = current.get(&entry.key);
        let after = updated.get(&entry.key);
        if entry.managed && before != after {
            errors.push(FieldError {
                key: entry.key.clone(),
                message: "is set by the app and can't be changed here".into(),
            });
            continue;
        }
        if let Some(value) = after
            && before != Some(value)
            && let Err(message) = entry.validate(value)
        {
            errors.push(FieldError {
                key: entry.key.clone(),
                message,
            });
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(PropertiesError::Invalid(errors))
    }
}

fn invalid(key: &str, message: &str) -> PropertiesError {
    PropertiesError::Invalid(vec![FieldError {
        key: key.into(),
        message: message.into(),
    }])
}

fn snapshot(bytes: &[u8], exists: bool) -> PropertiesSnapshot {
    let doc = PropertiesDoc::from_bytes(bytes);
    PropertiesSnapshot {
        revision: revision(bytes),
        exists,
        text: doc.render(),
        values: doc
            .entries()
            .into_iter()
            .map(|(key, value)| KeyValue {
                key: key.into(),
                value: value.into(),
            })
            .collect(),
    }
}

fn revision(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn file() -> Result<SafeRelPath, PropertiesError> {
    Ok(SafeRelPath::parse(FILE).map_err(FilesError::from)?)
}

/// The file's bytes, or `None` when there is none yet.
async fn read_bytes(files: &ServerFiles) -> Result<Option<Vec<u8>>, PropertiesError> {
    Ok(files.read_optional(file()?, MAX_FILE_BYTES as u64).await?)
}

#[cfg(test)]
mod tests;
