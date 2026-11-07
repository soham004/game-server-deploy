//! Archives inside a server directory: extract (`.zip`, `.tar`, `.tar.gz`/`.tgz`), compress a
//! selection to `.zip`, and stream a folder as a zip for download.
//!
//! Archive entry names are untrusted: each goes through [`SafeRelPath`] (no `..`, no absolute
//! paths: "zip slip"), links and special files are skipped, and the total size and entry count
//! are capped by the bytes actually written, not what the headers claim (zip bombs).

use std::io::{self, Read, Seek, Write};

use cap_std::fs::OpenOptions;
use serde::Serialize;
use zip::{CompressionMethod, write::SimpleFileOptions};

use super::{FilesError, SafeRelPath, root::Root};

/// How much one extraction may unpack (protects the disk from zip bombs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractLimits {
    pub max_bytes: u64,
    pub max_entries: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024 * 1024,
            max_entries: 200_000,
        }
    }
}

/// What an extraction did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ExtractSummary {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    /// Links, devices and entries with unsafe names that were left out.
    pub skipped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

// WHY: the name is lowercased first, so these comparisons are case-insensitive (`.tar.gz` is a
// double extension that `Path::extension` can't express).
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn archive_kind(path: &SafeRelPath) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

/// Where extracted entries go, and how much is left of the limits.
struct Extraction<'a> {
    root: &'a Root,
    destination: SafeRelPath,
    limits: ExtractLimits,
    summary: ExtractSummary,
}

impl Root {
    /// Extracts `archive` into the folder `destination` (created if missing). Existing files with
    /// the same names are replaced (server packs are extracted over a server).
    pub(crate) fn extract(
        &self,
        archive: &SafeRelPath,
        destination: &SafeRelPath,
        limits: ExtractLimits,
    ) -> Result<ExtractSummary, FilesError> {
        let kind = archive_kind(archive).ok_or_else(|| {
            FilesError::InvalidOperation(format!("{archive} is not a .zip, .tar, .tar.gz or .tgz archive"))
        })?;
        self.ensure_dir(destination)?;
        let file = self
            .dir
            .open(archive.as_path())
            .map_err(|e| FilesError::io(archive, e))?
            .into_std();
        let mut extraction = Extraction {
            root: self,
            destination: destination.clone(),
            limits,
            summary: ExtractSummary::default(),
        };
        let bad = |detail: String| FilesError::BadArchive(archive.clone(), detail);
        match kind {
            ArchiveKind::Zip => extraction.zip(file).map_err(|e| e.into_files_error(bad))?,
            ArchiveKind::Tar => extraction.tar(file).map_err(|e| e.into_files_error(bad))?,
            ArchiveKind::TarGz => extraction
                .tar(flate2::read::GzDecoder::new(file))
                .map_err(|e| e.into_files_error(bad))?,
        }
        Ok(extraction.summary)
    }

    /// Zips `sources` (files or folders, each stored under its own name) into the new file
    /// `destination`, which must end in `.zip`.
    pub(crate) fn compress(
        &self,
        sources: &[SafeRelPath],
        destination: &SafeRelPath,
    ) -> Result<(), FilesError> {
        if archive_kind(destination) != Some(ArchiveKind::Zip) {
            return Err(FilesError::InvalidOperation(
                "the archive name must end in .zip".into(),
            ));
        }
        if sources.is_empty() {
            return Err(FilesError::InvalidOperation("choose what to compress".into()));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let file = self
            .dir
            .open_with(destination.as_path(), &options)
            .map_err(|e| FilesError::io(destination, e))?
            .into_std();
        let written = (|| {
            let mut zip = zip::ZipWriter::new(file);
            for source in sources {
                if destination.starts_with(source) {
                    return Err(FilesError::InvalidOperation(format!(
                        "{destination} can't be inside {source}"
                    )));
                }
                let name = source.file_name().ok_or_else(|| {
                    FilesError::InvalidOperation("the server folder can't be compressed into itself".into())
                })?;
                self.add_to_zip(&mut zip, source, name)?;
            }
            zip.finish()
                .map_err(|e| FilesError::io(destination, io::Error::other(e)))?;
            Ok(())
        })();
        if let Err(error) = written {
            let _ = self.dir.remove_file(destination.as_path());
            return Err(error);
        }
        self.own(destination)
    }

    /// Writes `path` (a folder) as a zip to `writer`, which may not seek (a download stream).
    pub(crate) fn zip_folder_to(&self, path: &SafeRelPath, writer: impl Write) -> Result<(), FilesError> {
        let mut zip = zip::ZipWriter::new_stream(writer);
        let name = path.file_name().unwrap_or("server");
        self.add_to_zip(&mut zip, path, name)?;
        zip.finish()
            .map_err(|e| FilesError::io(path, io::Error::other(e)))?;
        Ok(())
    }

    /// Adds `source` to `zip` as `name` (recursively for folders; links are skipped).
    fn add_to_zip<W: Write + Seek>(
        &self,
        zip: &mut zip::ZipWriter<W>,
        source: &SafeRelPath,
        name: &str,
    ) -> Result<(), FilesError> {
        let zip_error = |e: zip::result::ZipError| FilesError::io(source, io::Error::other(e));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let metadata = self
            .dir
            .symlink_metadata(source.as_path())
            .map_err(|e| FilesError::io(source, e))?;
        if metadata.is_symlink() {
            return Ok(());
        }
        if metadata.is_dir() {
            zip.add_directory(format!("{name}/"), options)
                .map_err(zip_error)?;
            for entry in self.list(source)? {
                self.add_to_zip(zip, &entry.path, &format!("{name}/{}", entry.name))?;
            }
            return Ok(());
        }
        zip.start_file(name, options).map_err(zip_error)?;
        let mut file = self
            .dir
            .open(source.as_path())
            .map_err(|e| FilesError::io(source, e))?;
        io::copy(&mut file, zip).map_err(|e| FilesError::io(source, e))?;
        Ok(())
    }

    fn ensure_dir(&self, path: &SafeRelPath) -> Result<(), FilesError> {
        if path.is_root() {
            return Ok(());
        }
        match self.dir.symlink_metadata(path.as_path()) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(FilesError::NotADirectory(path.clone())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    self.ensure_dir(&parent)?;
                }
                self.create_dir(path)
            }
            Err(error) => Err(FilesError::io(path, error)),
        }
    }
}

/// Extraction failures: the archive is malformed, or a file operation failed.
enum ExtractError {
    Archive(String),
    Files(FilesError),
}

impl ExtractError {
    fn into_files_error(self, bad: impl FnOnce(String) -> FilesError) -> FilesError {
        match self {
            Self::Archive(detail) => bad(detail),
            Self::Files(error) => error,
        }
    }
}

impl From<FilesError> for ExtractError {
    fn from(error: FilesError) -> Self {
        Self::Files(error)
    }
}

impl Extraction<'_> {
    fn zip(&mut self, file: impl Read + Seek) -> Result<(), ExtractError> {
        let mut archive = zip::ZipArchive::new(file).map_err(|e| ExtractError::Archive(e.to_string()))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| ExtractError::Archive(e.to_string()))?;
            let is_link = entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000);
            let name = entry
                .name()
                .map_err(|e| ExtractError::Archive(e.to_string()))?
                .into_owned();
            let Some(path) = self.target(&name, is_link)? else {
                continue;
            };
            if entry.is_dir() {
                self.folder(&path)?;
            } else {
                self.file(&path, &mut entry)?;
            }
        }
        Ok(())
    }

    fn tar(&mut self, reader: impl Read) -> Result<(), ExtractError> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive
            .entries()
            .map_err(|e| ExtractError::Archive(e.to_string()))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| ExtractError::Archive(e.to_string()))?;
            let kind = entry.header().entry_type();
            let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
            let supported = kind.is_file() || kind.is_dir();
            let Some(path) = self.target(&name, !supported)? else {
                continue;
            };
            if kind.is_dir() {
                self.folder(&path)?;
            } else {
                self.file(&path, &mut entry)?;
            }
        }
        Ok(())
    }

    /// Where an entry goes, or `None` (counted as skipped) for links and unsafe names.
    fn target(&mut self, name: &str, skip: bool) -> Result<Option<SafeRelPath>, ExtractError> {
        let seen = self.summary.files + self.summary.folders + self.summary.skipped;
        if seen >= self.limits.max_entries {
            return Err(ExtractError::Archive(format!(
                "more than {} entries",
                self.limits.max_entries
            )));
        }
        let relative = SafeRelPath::parse(name).ok().filter(|p| !p.is_root());
        let joined = relative.and_then(|rel| {
            let mut path = self.destination.clone();
            for part in rel.as_str().split('/') {
                path = path.join(part).ok()?;
            }
            Some(path)
        });
        match joined {
            Some(path) if !skip => Ok(Some(path)),
            _ => {
                self.summary.skipped += 1;
                Ok(None)
            }
        }
    }

    fn folder(&mut self, path: &SafeRelPath) -> Result<(), ExtractError> {
        self.root.ensure_dir(path)?;
        self.summary.folders += 1;
        Ok(())
    }

    fn file(&mut self, path: &SafeRelPath, contents: &mut impl Read) -> Result<(), ExtractError> {
        if let Some(parent) = path.parent() {
            self.root.ensure_dir(&parent)?;
        }
        let remaining = self.limits.max_bytes.saturating_sub(self.summary.bytes);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        let mut file = self
            .root
            .dir
            .open_with(path.as_path(), &options)
            .map_err(|e| FilesError::io(path, e))?;
        let written = io::copy(&mut contents.take(remaining + 1), &mut file)
            .map_err(|e| ExtractError::Archive(format!("could not read {path} from the archive: {e}")))?;
        if written > remaining {
            return Err(ExtractError::Archive(format!(
                "it unpacks to more than {} bytes",
                self.limits.max_bytes
            )));
        }
        self.root.own(path)?;
        self.summary.files += 1;
        self.summary.bytes += written;
        Ok(())
    }
}
