//! Where an instance's files live, and who owns them.
//!
//! ```text
//! <data>/instances/<slug>/          root:root 0711  (the server can't rename or replace files here)
//!   instance.toml                   root:root 0600  (definition; never writable by the server)
//!   server/                         uid:gid   0700  (the game's working directory)
//!   home/                           uid:gid   0700  ($HOME / user.home)
//!   tmp/                            uid:gid   0700  ($TMPDIR / java.io.tmpdir)
//! ```

use std::path::{Path, PathBuf};

use crate::instances::SandboxIds;

pub const DEFINITION_FILE: &str = "instance.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceLayout {
    root: PathBuf,
}

impl InstanceLayout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn slug(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn definition_path(&self) -> PathBuf {
        self.root.join(DEFINITION_FILE)
    }

    /// The game's working directory (what the file manager shows).
    pub fn server_dir(&self) -> PathBuf {
        self.root.join("server")
    }

    pub fn home_dir(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Files deleted in the file manager and what a restore set aside (outside the server's
    /// reach; emptied by `trash::purge`).
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }

    /// Directories the server process may write to.
    pub fn writable_dirs(&self) -> [PathBuf; 3] {
        [self.server_dir(), self.home_dir(), self.tmp_dir()]
    }

    /// Creates the directory tree with the ownership and modes shown in the module docs.
    /// Ownership changes need root; without it (development) the current user keeps ownership.
    pub fn create(&self, ids: SandboxIds) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        set_mode(&self.root, 0o711)?;
        for dir in self.writable_dirs() {
            std::fs::create_dir_all(&dir)?;
            set_mode(&dir, 0o700)?;
        }
        self.assign_ownership(ids)
    }

    /// Gives the sandbox user ownership of everything under the writable directories
    /// (call after installing or extracting files into `server/`).
    pub fn assign_ownership(&self, ids: SandboxIds) -> std::io::Result<()> {
        if !can_change_ownership() {
            return Ok(());
        }
        for dir in self.writable_dirs() {
            chown_recursive(&dir, ids)?;
        }
        Ok(())
    }
}

/// Gives one file (or directory, not recursively) to the sandbox user, e.g. after the app
/// replaced a file in `server/`. A no-op without root. Never follows a symlink.
pub fn assign_file_ownership(path: &Path, ids: SandboxIds) -> std::io::Result<()> {
    if !can_change_ownership() {
        return Ok(());
    }
    #[cfg(unix)]
    std::os::unix::fs::lchown(path, Some(ids.uid), Some(ids.gid))?;
    #[cfg(not(unix))]
    let _ = (path, ids);
    Ok(())
}

/// Turns a display name into a directory-safe slug: "My Server!" → "my-server".
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        "server".to_owned()
    } else {
        slug
    }
}

/// True when this process may `chown` files (root on Unix).
pub fn can_change_ownership() -> bool {
    #[cfg(unix)]
    {
        nix::unistd::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Gives `path` and everything beneath it to `ids`.
///
/// The tree belongs to a server that may be compromised, and something it started may still run.
/// So nothing is reached by its full path, which could be redirected by swapping a folder for a
/// link between two steps. Each folder is opened beneath the top one (`cap-std` refuses a way out
/// of it) and its entries are changed by name through that handle, never following a link.
fn chown_recursive(path: &Path, ids: SandboxIds) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use cap_std::{ambient_authority, fs::Dir};
        use rustix::fs::{AtFlags, Gid, Uid, chownat};

        let gone = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
        // The top folder's parent is the app's own, so its name can't be swapped.
        std::os::unix::fs::lchown(path, Some(ids.uid), Some(ids.gid))?;
        let top = Dir::open_ambient_dir(path, ambient_authority())?;
        // Folders still to walk, as paths beneath `top` (one handle open at a time).
        let mut pending = vec![PathBuf::new()];
        while let Some(folder) = pending.pop() {
            let dir = if folder.as_os_str().is_empty() {
                top.try_clone()?
            } else {
                match top.open_dir(&folder) {
                    Ok(dir) => dir,
                    Err(error) if gone(&error) => continue,
                    Err(error) => return Err(error),
                }
            };
            for entry in dir.entries()? {
                let entry = entry?;
                let name = entry.file_name();
                let owned = chownat(
                    &dir,
                    &name,
                    Some(Uid::from_raw(ids.uid)),
                    Some(Gid::from_raw(ids.gid)),
                    AtFlags::SYMLINK_NOFOLLOW,
                )
                .map_err(std::io::Error::from);
                match owned {
                    Ok(()) => {}
                    Err(error) if gone(&error) => continue,
                    Err(error) => return Err(error),
                }
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(folder.join(&name));
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (path, ids);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_directory_safe() {
        assert_eq!(slugify("My Server!"), "my-server");
        assert_eq!(slugify("  ATM 10 -- Modded  "), "atm-10-modded");
        assert_eq!(slugify("日本語"), "server");
        assert_eq!(slugify(&"x".repeat(80)).len(), 40);
        assert!(!slugify(&format!("{}-tail", "a".repeat(39))).ends_with('-'));
    }

    #[test]
    fn creates_the_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstanceLayout::new(dir.path().join("survival"));
        layout
            .create(SandboxIds {
                uid: 20000,
                gid: 20000,
            })
            .unwrap();
        assert!(layout.server_dir().is_dir() && layout.home_dir().is_dir() && layout.tmp_dir().is_dir());
        assert_eq!(layout.slug(), "survival");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(layout.root()), 0o711);
            assert_eq!(mode(&layout.server_dir()), 0o700);
        }
    }

    /// As root the tree goes to a server's user; otherwise to the caller (all it may set), which
    /// still walks the same way. Either way a link is changed itself, never what it points at.
    #[cfg(unix)]
    #[test]
    fn ownership_is_given_inside_the_tree_and_never_through_a_link() {
        use std::os::unix::fs::{MetadataExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let (tree, outside) = (dir.path().join("server"), dir.path().join("outside"));
        std::fs::create_dir_all(tree.join("world/region")).unwrap();
        std::fs::write(tree.join("world/region/r.0.0.mca"), "x").unwrap();
        std::fs::write(tree.join("server.properties"), "x").unwrap();
        std::fs::create_dir_all(outside.join("inner")).unwrap();
        std::fs::write(outside.join("inner/secret"), "x").unwrap();
        symlink(&outside, tree.join("link-to-folder")).unwrap();
        symlink(outside.join("inner/secret"), tree.join("world/link-to-file")).unwrap();
        symlink("/nonexistent", tree.join("dangling")).unwrap();

        let owner_of = |path: &Path| std::fs::symlink_metadata(path).unwrap().uid();
        let before = owner_of(&outside);
        let ids = if nix::unistd::geteuid().is_root() {
            SandboxIds {
                uid: 20123,
                gid: 20123,
            }
        } else {
            SandboxIds {
                uid: nix::unistd::geteuid().as_raw(),
                gid: nix::unistd::getegid().as_raw(),
            }
        };

        chown_recursive(&tree, ids).unwrap();

        for inside in [
            "",
            "world",
            "world/region",
            "world/region/r.0.0.mca",
            "server.properties",
            "link-to-folder",
            "world/link-to-file",
            "dangling",
        ] {
            assert_eq!(owner_of(&tree.join(inside)), ids.uid, "{inside}");
        }
        for untouched in [
            outside.clone(),
            outside.join("inner"),
            outside.join("inner/secret"),
        ] {
            assert_eq!(owner_of(&untouched), before, "{}", untouched.display());
        }
    }
}
