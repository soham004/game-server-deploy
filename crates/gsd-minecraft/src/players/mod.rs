//! Players: who is on, operators, the allowlist, bans, everyone seen, and their heads.
//!
//! - [`view`]: the page's data, from the server's own files ([`lists`]), the console's list of who is on,
//!   and the visit history (`gsd_core::players`).
//! - [`action`]: what can be done, checked; [`service`]: doing it the way the server's state allows.
//! - [`names`]: names and UUIDs, offline mode included; [`mojang`]: accounts and skins; [`head`]: a head
//!   from a skin.

pub mod action;
pub mod head;
pub mod lists;
pub mod mojang;
pub mod names;
pub mod service;
pub mod view;

pub use action::PlayerAction;
pub use mojang::{Mojang, MojangError, ProfileEndpoints};
pub use service::{Applied, PlayerService, PlayersError};
pub use view::{ActionChannel, PlayersView};
