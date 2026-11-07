//! What can be installed: Minecraft versions and, per version, which server software ("loaders")
//! and loader versions exist, and what to download for each.
//!
//! [`Catalog`] (`queries.rs`) dispatches each question to one module per source: `mojang`,
//! `paper`, `purpur`, `fabric`, `quilt`, `forge`, `neoforge`. Responses are cached in memory and
//! on disk (`fetch.rs`), so the picker keeps working when an upstream is down.

mod fabric;
mod fetch;
mod forge;
mod maven;
mod model;
mod mojang;
pub(crate) mod neoforge;
mod paper;
mod purpur;
mod queries;
mod quilt;
pub mod version_order;

#[cfg(test)]
pub(crate) mod tests;

use fetch::Fetcher;
pub use model::{Artifact, CatalogError, Endpoints, GameVersion, LoaderAvailability, LoaderVersion};
pub use mojang::{ServerDownload, VersionDetail, VersionKind};
pub use queries::Catalog;
