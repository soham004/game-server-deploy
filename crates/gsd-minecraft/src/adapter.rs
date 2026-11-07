//! The Minecraft implementation of the core's [`GameAdapter`].

use gsd_core::game::{Diagnostic, GameAdapter, GameSignal, LineSource, LogLine, ParsedLine};

use crate::{diagnostics, log_parser};

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
            Some("port_in_use")
        );
        assert!(
            MinecraftAdapter
                .diagnose(&line(LineSource::System, bind))
                .is_none()
        );
        assert_eq!(MinecraftAdapter.stop_command(), "stop");
    }
}
