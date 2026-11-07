//! The seam between the game-agnostic core and a game adapter (today: Minecraft).
//!
//! The supervisor only needs a game to tell it how to read its console output and how to ask
//! it to stop. Everything else game-specific (versions, installers) lives in the adapter crate.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Severity of a console line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    /// The line didn't carry a level (e.g. a stack-trace continuation).
    Unknown,
}

/// Where a console line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineSource {
    Stdout,
    Stderr,
    /// Echo of a command someone sent.
    Input,
    /// A message from the app itself (start, stop, crash notices).
    System,
}

/// One parsed console line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogLine {
    /// Monotonic per-process sequence number, so clients can de-duplicate after a resync.
    pub seq: u64,
    pub received_at: DateTime<Utc>,
    pub source: LineSource,
    pub level: LogLevel,
    /// Thread name, if the line format carries one (e.g. "Server thread").
    pub thread: Option<String>,
    /// The message without the timestamp/thread/level prefix.
    pub text: String,
    /// The line exactly as printed.
    pub raw: String,
}

/// The parts of a line a game adapter extracts (the supervisor adds sequence and time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub level: LogLevel,
    pub thread: Option<String>,
    pub text: String,
}

/// Something the game told us through its output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GameSignal {
    /// The server finished starting and accepts players.
    Ready,
    PlayerJoined {
        name: String,
    },
    PlayerLeft {
        name: String,
    },
    /// A world save completed.
    Saved,
}

/// A recognised problem with a human explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// Stable code the UI can attach actions to, e.g. `"port_bind_failed"`. One diagnostic per
    /// code: a later one with the same code replaces it (a more specific explanation).
    pub code: &'static str,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// What the supervisor needs from a game.
pub trait GameAdapter: Send + Sync + std::fmt::Debug {
    /// Splits a console line into level, thread and message.
    fn parse_line(&self, raw: &str) -> ParsedLine;
    /// Recognises lifecycle and player events.
    fn detect_signal(&self, line: &LogLine) -> Option<GameSignal>;
    /// Recognises known problems (wrong Java, port in use, …).
    fn diagnose(&self, line: &LogLine) -> Option<Diagnostic>;
    /// The console command that stops the server gracefully.
    fn stop_command(&self) -> &str;
}
