//! The application: configuration, wiring, the HTTP API and the embedded web UI.
//!
//! `main.rs` only parses the command line and calls [`cli::run`]. Everything is in this library
//! so integration tests can build the real router (`http::router`) against a test database.

pub mod assets;
pub mod cli;
pub mod config;
pub mod http;
pub mod serve;
pub mod state;
