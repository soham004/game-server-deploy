//! Opening the files of a server folder for reading: regular files only, and without waiting.
//!
//! The game server owns the folder and may put a FIFO or a device where a file should be. Opening a
//! FIFO to read waits for a writer that never comes, which would hold a blocking thread for good,
//! and a device such as `/dev/zero` never ends. So every read opens without blocking, looks at what
//! it got, and takes at most what the caller allows (a sparse file claims any size for free).

use std::{
    io::{self, Read},
    path::Path,
};

use cap_std::fs::{Dir, File, OpenOptions};

/// Opens `path` beneath `dir` for reading. A folder is `IsADirectory`; anything else that isn't a
/// regular file (a FIFO, a socket, a device) is `InvalidInput`.
pub(crate) fn open_regular(dir: &Dir, path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    without_waiting(&mut options);
    let file = match dir.open_with(path, &options) {
        Ok(file) => file,
        // Where a folder can't be opened like a file (Windows), still say that it is one.
        Err(_) if dir.metadata(path).is_ok_and(|metadata| metadata.is_dir()) => {
            return Err(io::Error::from(io::ErrorKind::IsADirectory));
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.is_dir() {
        return Err(io::Error::from(io::ErrorKind::IsADirectory));
    }
    if !metadata.is_file() {
        return Err(not_a_regular_file());
    }
    Ok(file)
}

/// Makes opening a FIFO return at once instead of waiting for its other end (reading and writing
/// regular files is unchanged).
pub(crate) fn without_waiting(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        let nonblocking = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).unwrap_or_default();
        options.custom_flags(nonblocking);
    }
    #[cfg(not(unix))]
    let _ = options;
}

pub(crate) fn not_a_regular_file() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not a regular file")
}

/// The whole of `file` if it is at most `limit` bytes, else `None`. The size is what is actually
/// read, not what the file says.
pub(crate) fn read_at_most(file: &mut File, limit: u64) -> io::Result<Option<Vec<u8>>> {
    if file.metadata()?.len() > limit {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// Reads the file `name` at the top of `folder` when it is a regular file of at most `limit` bytes.
/// `None` when it is missing, too large, not a file, or a link out of the folder. For code that
/// only has the folder's path and wants a setting from it (the world's name).
pub fn read_small_file(folder: &Path, name: &str, limit: u64) -> Option<Vec<u8>> {
    let dir = Dir::open_ambient_dir(folder, cap_std::ambient_authority()).ok()?;
    let mut file = open_regular(&dir, Path::new(name)).ok()?;
    read_at_most(&mut file, limit).ok().flatten()
}
