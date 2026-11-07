//! Game-agnostic core services.
//!
//! This crate knows nothing about HTTP or Minecraft. It owns persistence (`store`), identity
//! and access control (`auth`), the audit trail (`audit`) and shared value types (`ids`,
//! `ports`). Later milestones add the process supervisor, jobs, scheduler and backups here.
//!
//! Flow rule (see `claude/plan/01-architecture.md` §3): HTTP handlers call one service
//! method; services orchestrate; repositories are the only code that touches SQL.

pub mod audit;
pub mod auth;
pub mod ids;
pub mod ports;
pub mod random;
pub mod store;
