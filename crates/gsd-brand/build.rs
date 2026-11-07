//! Generates `$OUT_DIR/brand.rs` from the build-time branding variables.
//!
//! This file is the **only** place the default product name, slug and port are written down
//! (see `claude/plan/07-deployment-and-branding.md`). Override them when building:
//!
//! ```text
//! APP_NAME="My Panel" APP_SLUG=mypanel APP_DEFAULT_PORT=9000 cargo build --release
//! ```
//!
//! An empty variable counts as unset, so Docker build args can default to "".

// WHY: a build script reports invalid configuration by panicking; that is the intended
// mechanism, so `expect`/`panic!` are allowed here.
#![allow(clippy::expect_used, clippy::panic)]

use std::{env, fs, path::PathBuf};

const DEFAULT_NAME: &str = "Game Server Deploy";
const DEFAULT_SLUG: &str = "gsd";
const DEFAULT_PORT: u16 = 8737;

fn main() {
    for var in ["APP_NAME", "APP_SLUG", "APP_DEFAULT_PORT"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let name = read_var("APP_NAME").unwrap_or_else(|| DEFAULT_NAME.to_owned());
    let slug = read_var("APP_SLUG").unwrap_or_else(|| DEFAULT_SLUG.to_owned());
    let port = read_var("APP_DEFAULT_PORT").map_or(DEFAULT_PORT, |raw| parse_port(&raw));

    validate_name(&name);
    validate_slug(&slug);

    let env_prefix = slug.to_ascii_uppercase().replace('-', "_");
    let session_cookie = format!("{}_session", slug.replace('-', "_"));

    let generated = format!(
        "/// Human-readable product name.\n\
         pub const NAME: &str = {name:?};\n\
         /// Short machine name: binary, data directory, systemd unit, image name.\n\
         pub const SLUG: &str = {slug:?};\n\
         /// Default web UI port (runtime configuration can override it).\n\
         pub const DEFAULT_PORT: u16 = {port};\n\
         /// Prefix of every environment variable the app reads, e.g. `{env_prefix}_DATA_DIR`.\n\
         pub const ENV_PREFIX: &str = {env_prefix:?};\n\
         /// Name of the session cookie.\n\
         pub const SESSION_COOKIE: &str = {session_cookie:?};\n"
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo always sets OUT_DIR"));
    fs::write(out_dir.join("brand.rs"), generated).expect("failed to write brand.rs");
}

fn read_var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_port(raw: &str) -> u16 {
    match raw.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => panic!("APP_DEFAULT_PORT must be a number between 1 and 65535, got {raw:?}"),
    }
}

fn validate_name(name: &str) {
    let valid = name.chars().count() <= 64 && !name.chars().any(char::is_control);
    assert!(
        valid,
        "APP_NAME must be at most 64 printable characters, got {name:?}"
    );
}

fn validate_slug(slug: &str) {
    let mut chars = slug.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_valid = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let valid = starts_with_letter && rest_valid && (2..=32).contains(&slug.len());
    assert!(valid, "APP_SLUG must match [a-z][a-z0-9-]{{1,31}}, got {slug:?}");
}
