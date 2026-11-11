//! Writing and reading backup archives.
//!
//! A backup is a zip holding the chosen top-level entries of the server folder under `server/`
//! and the [`Manifest`] as `gsd-backup.json`. It is written in one pass through a hashing writer,
//! so the SHA-256 is known the moment the file is complete. The server folder belongs to a game
//! server that may be compromised: it is read through `cap-std` handles, and symlinks and special
//! files are left out (as when cloning, see `files::copy_server_dir`).

use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    path::Path,
};

use cap_std::{ambient_authority, fs::Dir};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::model::Manifest;
use crate::files::regular::open_regular;

/// The manifest's name at the top of the archive.
pub const MANIFEST_NAME: &str = "gsd-backup.json";
/// The folder in the archive that holds the server's files.
pub const SERVER_FOLDER: &str = "server";

/// Which top-level entries of the server folder go into a backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Only these (a world backup).
    Only(Vec<String>),
    /// Everything except these (a full backup).
    AllExcept(Vec<String>),
}

impl Selection {
    pub fn includes(&self, name: &str) -> bool {
        match self {
            Self::Only(names) => names.iter().any(|n| n == name),
            Self::AllExcept(names) => !names.iter().any(|n| n == name),
        }
    }
}

/// A complete archive on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenArchive {
    pub size_bytes: u64,
    pub sha256: String,
    /// The manifest as stored, with the entries and totals filled in.
    pub manifest: Manifest,
}

/// Writes the backup of `server_dir` to the new file `destination`. `manifest` is completed
/// with what was archived. A partial file is removed on any error, including cancellation.
pub fn write_archive(
    server_dir: &Path,
    selection: &Selection,
    destination: &Path,
    manifest: Manifest,
    cancel: &CancellationToken,
) -> io::Result<WrittenArchive> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let written = write_into(file, server_dir, selection, manifest, cancel);
    if written.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    written
}

/// The bytes the selected files take before compression (a disk-space estimate).
pub fn measure(server_dir: &Path, selection: &Selection) -> io::Result<u64> {
    fn folder_size(dir: &Dir) -> io::Result<u64> {
        let mut total = 0;
        for entry in dir.entries()? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                total += folder_size(&dir.open_dir(entry.file_name())?)?;
            } else if file_type.is_file() {
                total += entry.metadata()?.len();
            }
        }
        Ok(total)
    }
    let source = Dir::open_ambient_dir(server_dir, ambient_authority())?;
    let mut total = 0;
    for name in top_level_names(&source, selection)? {
        let file_type = source.symlink_metadata(&name)?.file_type();
        if file_type.is_dir() {
            total += folder_size(&source.open_dir(&name)?)?;
        } else if file_type.is_file() {
            total += source.metadata(&name)?.len();
        }
    }
    Ok(total)
}

/// Reads the manifest of a backup archive.
pub fn read_manifest(archive: &Path) -> io::Result<Manifest> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(io::Error::other)?;
    let entry = zip.by_name(MANIFEST_NAME).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{MANIFEST_NAME} is missing: not a backup made by this app"),
        )
    })?;
    serde_json::from_reader(entry).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// SHA-256 of a file, hex.
pub fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

type Archive = ZipWriter<zip::write::StreamWriter<HashingWriter<BufWriter<File>>>>;

/// Counts of what went into the archive.
#[derive(Debug, Default)]
struct Totals {
    files: u64,
    folders: u64,
    bytes: u64,
}

fn write_into(
    file: File,
    server_dir: &Path,
    selection: &Selection,
    mut manifest: Manifest,
    cancel: &CancellationToken,
) -> io::Result<WrittenArchive> {
    let source = Dir::open_ambient_dir(server_dir, ambient_authority())?;
    let mut zip = ZipWriter::new_stream(HashingWriter::new(BufWriter::new(file)));
    let mut totals = Totals::default();
    let mut entries = Vec::new();
    for name in top_level_names(&source, selection)? {
        let path = format!("{SERVER_FOLDER}/{name}");
        if add_entry(&mut zip, &source, &name, &path, cancel, &mut totals)? {
            entries.push(name);
        }
    }

    manifest.entries = entries;
    manifest.files = totals.files;
    manifest.folders = totals.folders;
    manifest.bytes = totals.bytes;
    zip.start_file(MANIFEST_NAME, file_options(0))
        .map_err(io::Error::other)?;
    serde_json::to_writer_pretty(&mut zip, &manifest).map_err(io::Error::other)?;

    let (buffered, sha256, size_bytes) = zip.finish().map_err(io::Error::other)?.into_inner().finish();
    buffered
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()?;
    Ok(WrittenArchive {
        size_bytes,
        sha256,
        manifest,
    })
}

/// The selected top-level entry names, sorted (archives list entries in a stable order).
fn top_level_names(source: &Dir, selection: &Selection) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in source.entries()? {
        // Names that aren't UTF-8 can't be stored portably; the game never creates them.
        if let Ok(name) = entry?.file_name().into_string()
            && selection.includes(&name)
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Adds `name` (in `dir`) to the archive at `path`, recursing into folders. Returns false for a
/// symlink or special file, which is left out.
fn add_entry(
    zip: &mut Archive,
    dir: &Dir,
    name: &str,
    path: &str,
    cancel: &CancellationToken,
    totals: &mut Totals,
) -> io::Result<bool> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let metadata = dir.symlink_metadata(name)?;
    if metadata.is_dir() {
        zip.add_directory(format!("{path}/"), file_options(0))
            .map_err(io::Error::other)?;
        totals.folders += 1;
        let child = dir.open_dir(name)?;
        let mut names: Vec<String> = child
            .entries()?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        names.sort();
        for child_name in names {
            add_entry(
                zip,
                &child,
                &child_name,
                &format!("{path}/{child_name}"),
                cancel,
                totals,
            )?;
        }
        return Ok(true);
    }
    if !metadata.is_file() {
        return Ok(false);
    }
    // Opened before its entry is started: the server may have swapped it for a FIFO since the
    // look above, and that is then left out like any other special file.
    let mut file = match open_regular(dir, Path::new(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => return Ok(false),
        Err(error) => return Err(error),
    };
    zip.start_file(path, file_options(metadata.len()))
        .map_err(io::Error::other)?;
    totals.bytes += io::copy(&mut file, zip)?;
    totals.files += 1;
    Ok(true)
}

fn file_options(size: u64) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        // Zip64 headers for entries of 4 GiB or more (large region or database files).
        .large_file(size >= u64::from(u32::MAX))
}

/// Passes writes through while hashing them and counting bytes.
#[derive(Debug)]
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            written: 0,
        }
    }

    /// The inner writer, the hex SHA-256 and the byte count.
    fn finish(self) -> (W, String, u64) {
        (self.inner, hex::encode(self.hasher.finalize()), self.written)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(buf)?;
        self.hasher.update(&buf[..count]);
        self.written += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
