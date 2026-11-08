//! The Minecraft implementation of the core's [`GameAdapter`].

use std::path::Path;

use gsd_core::game::{Diagnostic, GameAdapter, GameSignal, LineSource, LogLine, ParsedLine, SaveCommands};

use crate::{diagnostics, log_parser, settings::MinecraftServer, worlds};

/// Game id stored in `instance.toml`.
pub const GAME_ID: &str = "minecraft";

#[derive(Debug, Default, Clone, Copy)]
pub struct MinecraftAdapter;

impl GameAdapter for MinecraftAdapter {
    fn parse_line(&self, raw: &str) -> ParsedLine {
        log_parser::parse_line(raw)
    }

    fn detect_signal(&self, line: &LogLine) -> Option<GameSignal> {
        // Only the server's own output counts; echoed input or our notices must not fake events.
        if !matches!(line.source, LineSource::Stdout | LineSource::Stderr) {
            return None;
        }
        log_parser::detect_signal(&line.text)
    }

    fn diagnose(&self, line: &LogLine) -> Option<Diagnostic> {
        if !matches!(line.source, LineSource::Stdout | LineSource::Stderr) {
            return None;
        }
        diagnostics::diagnose(&line.raw)
    }

    fn stop_command(&self) -> &'static str {
        "stop"
    }

    fn software(&self, server: &toml::Table) -> Option<String> {
        MinecraftServer::from_table(server)
            .ok()
            .map(|server| server.software_label())
    }

    fn world_folders(&self, server_dir: &Path) -> Vec<String> {
        worlds::world_folders(server_dir)
    }

    fn backup_excludes(&self) -> &[&str] {
        // Logs, and what the server or its launcher recreates: downloaded libraries, the
        // patched or remapped jars (Paper's `cache/` and `versions/`, Fabric's `.fabric/`).
        &[
            "logs",
            "crash-reports",
            "cache",
            "libraries",
            "versions",
            ".fabric",
        ]
    }

    fn broadcast_command(&self, message: &str) -> Option<String> {
        Some(format!("say {message}"))
    }

    fn save_commands(&self) -> Option<SaveCommands> {
        Some(SaveCommands {
            pause: "save-off",
            // `flush` waits for the chunk writes to finish before "Saved the game" is printed.
            flush: "save-all flush",
            resume: "save-on",
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use gsd_core::game::LogLevel;

    use super::*;

    fn line(source: LineSource, raw: &str) -> LogLine {
        let parsed = MinecraftAdapter.parse_line(raw);
        LogLine {
            seq: 1,
            received_at: Utc::now(),
            source,
            level: parsed.level,
            thread: parsed.thread,
            text: parsed.text,
            raw: raw.into(),
        }
    }

    #[test]
    fn only_server_output_produces_signals() {
        let done = r#"[12:00:00] [Server thread/INFO]: Done (1.0s)! For help, type "help""#;
        assert_eq!(
            MinecraftAdapter.detect_signal(&line(LineSource::Stdout, done)),
            Some(GameSignal::Ready)
        );
        assert_eq!(
            MinecraftAdapter.detect_signal(&line(LineSource::Input, done)),
            None
        );
        assert_eq!(line(LineSource::Stdout, done).level, LogLevel::Info);
    }

    #[test]
    fn diagnoses_server_output() {
        let bind = "[12:00:00] [Server thread/WARN]: **** FAILED TO BIND TO PORT!";
        assert_eq!(
            MinecraftAdapter
                .diagnose(&line(LineSource::Stdout, bind))
                .map(|d| d.code),
            Some("port_bind_failed")
        );
        assert!(
            MinecraftAdapter
                .diagnose(&line(LineSource::System, bind))
                .is_none()
        );
        assert_eq!(MinecraftAdapter.stop_command(), "stop");
    }
}
