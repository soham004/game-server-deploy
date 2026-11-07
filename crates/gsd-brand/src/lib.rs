//! Product branding: the single source of truth for the app's name, slug and default port.
//!
//! The values are fixed at build time by `build.rs` (from `APP_NAME`, `APP_SLUG` and
//! `APP_DEFAULT_PORT`). No other crate, file or test may hard-code them; reference these
//! constants instead. CI enforces this with `scripts/check-branding.sh`.

include!(concat!(env!("OUT_DIR"), "/brand.rs"));

/// Application version (shared by every crate in the workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full environment variable name for an app setting, e.g. `env_var("DATA_DIR")` → `GSD_DATA_DIR`.
pub fn env_var(name: &str) -> String {
    format!("{ENV_PREFIX}_{name}")
}

/// `User-Agent` sent to every external service (Mojang, Modrinth, `PaperMC`, …).
pub fn user_agent() -> String {
    format!("{SLUG}/{VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_uses_the_prefix() {
        assert_eq!(env_var("DATA_DIR"), format!("{ENV_PREFIX}_DATA_DIR"));
    }

    #[test]
    fn env_prefix_is_derived_from_the_slug() {
        assert_eq!(ENV_PREFIX, SLUG.to_ascii_uppercase().replace('-', "_"));
    }

    #[test]
    fn user_agent_names_product_and_version() {
        assert!(user_agent().starts_with(SLUG));
        assert!(user_agent().ends_with(VERSION));
    }
}
