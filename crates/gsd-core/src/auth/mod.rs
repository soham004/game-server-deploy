//! Identity and access control: users, sessions, roles and per-instance grants.
//!
//! - [`AuthService`] is the entry point used by the HTTP layer (setup, login, authenticate…).
//! - [`Actor`] is the authenticated user plus their effective permissions; every permission
//!   check goes through it.
//! - [`permissions`] defines the RBAC model (see `claude/plan/features/security-and-auth.md`).

mod actor;
mod error;
pub mod grants;
pub mod password;
pub mod permissions;
mod service;
pub mod sessions;
mod username;
pub mod users;

pub use actor::Actor;
pub use error::AuthError;
pub use permissions::{GlobalPermission, GlobalRole, Grant, Permission, PermissionSet};
pub use service::{AccountChange, AuthService, ClientInfo, LoginOutcome, UserSummary};
pub use username::Username;
pub use users::User;
