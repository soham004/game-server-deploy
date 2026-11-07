//! The data-dir sandbox check with the real probe (the `gsd` binary) and the real kernel.
//! Skipped where Landlock is missing, or when running as root (the probe would then run as
//! another user, who can't reach the build directory).

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use gsd_core::supervisor::{SandboxSupport, sandbox::check};

fn usable_support() -> Option<SandboxSupport> {
    let support = SandboxSupport::detect();
    if !support.filesystem || support.separate_users {
        eprintln!("skipping: needs Landlock and a non-root user");
        return None;
    }
    Some(support)
}

#[tokio::test]
async fn a_server_can_use_its_own_folder_on_a_local_filesystem() {
    let Some(support) = usable_support() else { return };
    let data = tempfile::tempdir().unwrap();
    let probe = Path::new(env!("CARGO_BIN_EXE_gsd"));

    let usable = check::own_folder_usable(probe, &data.path().join("sandbox-check"), support)
        .await
        .unwrap();
    assert!(
        usable,
        "the sandbox should allow a server's own folder on {}",
        data.path().display()
    );
}

#[tokio::test]
async fn the_check_cleans_up_after_itself() {
    let Some(support) = usable_support() else { return };
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().join("sandbox-check");
    let probe = Path::new(env!("CARGO_BIN_EXE_gsd"));

    check::own_folder_usable(probe, &dir, support).await.unwrap();
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "scratch folders are removed"
    );
}
