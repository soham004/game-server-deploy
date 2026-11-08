//! Game-agnostic core services.
//!
//! This crate knows nothing about HTTP or Minecraft:
//! - persistence (`store`), identity and access control (`auth`), the audit trail (`audit`);
//! - the event bus (`events`) and background jobs (`jobs`);
//! - instances on disk (`instances`), Java runtimes (`java`), and running, sandboxed server
//!   processes (`supervisor`);
//! - the seam to a game adapter (`game`).
//!
//! Flow rule (see `claude/plan/01-architecture.md` §3): HTTP handlers call one service
//! method; services orchestrate; repositories and adapters do the IO.

// Ownership, permissions and the sandbox are Unix/Linux work. Elsewhere those functions keep their
// `Result` signature but do nothing, which this pedantic lint would flag one by one.
#![cfg_attr(not(target_os = "linux"), allow(clippy::unnecessary_wraps))]

pub mod audit;
pub mod auth;
pub mod backups;
pub mod events;
pub mod files;
pub mod game;
pub mod ids;
pub mod instances;
pub mod java;
pub mod jobs;
pub mod net;
pub mod ports;
pub mod random;
pub mod scheduler;
pub mod store;
pub mod supervisor;
pub mod trash;
