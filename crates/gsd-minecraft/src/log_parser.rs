//! Parsing Minecraft console lines and recognising events in them.
//!
//! Formats seen in the wild:
//! ```text
//! [12:01:22] [Server thread/INFO]: Done (3.421s)! For help, type "help"        vanilla
//! [12:01:22 INFO]: Done (7.015s)! For help, type "help"                        Paper / Spigot
//! [12:01:22] [main/INFO] (FabricLoader/GameProvider) Loading Minecraft 26.3    Fabric
//! [12:01:22] [main/INFO] [cpw.mods.modlauncher.Launcher/MODLAUNCHER]: …        Forge / NeoForge
//! ```
//! Anything else (stack traces, banners) is kept verbatim with an unknown level.

use std::sync::LazyLock;

use gsd_core::game::{GameSignal, LogLevel, ParsedLine};
use regex::Regex;

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^
        \[\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:\x20(?P<plain_level>[A-Z]+))?\]   # [12:01:22] or [12:01:22 INFO]
        (?:\x20\[(?P<thread>[^\]]+?)/(?P<level>[A-Z]+)\])?                # [Server thread/INFO]
        (?:\x20\[[^\]]+\])?                                              # [logger] (Forge)
        (?:\x20\([^)]+\))?                                               # (logger) (Fabric)
        :?\x20?(?P<message>.*)$",
    )
    .unwrap_or_else(|error| unreachable!("static regex: {error}"))
});

static DONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^Done \([\d.,]+s\)! For help, type "help""#)
        .unwrap_or_else(|e| unreachable!("static regex: {e}"))
});

static JOINED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<name>[A-Za-z0-9_]{1,16}) joined the game$")
        .unwrap_or_else(|e| unreachable!("static regex: {e}"))
});

static LEFT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<name>[A-Za-z0-9_]{1,16}) left the game$")
        .unwrap_or_else(|e| unreachable!("static regex: {e}"))
});

/// Strips ANSI colour sequences some servers print.
static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap_or_else(|e| unreachable!("static regex: {e}"))
});

/// The line without colour sequences.
pub fn strip_ansi(raw: &str) -> std::borrow::Cow<'_, str> {
    ANSI.replace_all(raw, "")
}

pub fn parse_line(raw: &str) -> ParsedLine {
    let without_ansi = ANSI.replace_all(raw, "");
    // JLine (Paper's console) prefixes lines with its "> " prompt when stdout isn't a terminal.
    let clean = strip_prompt(&without_ansi);
    let Some(captures) = LINE.captures(clean) else {
        return ParsedLine {
            level: LogLevel::Unknown,
            thread: None,
            text: without_ansi.into_owned(),
        };
    };
    let level_text = captures
        .name("level")
        .or_else(|| captures.name("plain_level"))
        .map(|m| m.as_str());
    ParsedLine {
        level: level_text.map_or(LogLevel::Unknown, level_from),
        thread: captures.name("thread").map(|m| m.as_str().to_owned()),
        text: captures
            .name("message")
            .map_or_else(String::new, |m| m.as_str().to_owned()),
    }
}

pub fn detect_signal(text: &str) -> Option<GameSignal> {
    if DONE.is_match(text) {
        return Some(GameSignal::Ready);
    }
    let message = command_reply(text);
    if message == "Saved the game" {
        return Some(GameSignal::Saved);
    }
    if let Some(captures) = JOINED.captures(message) {
        return Some(GameSignal::PlayerJoined {
            name: captures["name"].to_owned(),
        });
    }
    LEFT.captures(message).map(|captures| GameSignal::PlayerLeft {
        name: captures["name"].to_owned(),
    })
}

/// A system message without its prefix: Minecraft 26.x logs command replies and announcements such as
/// joins as `System chat: Saved the game`; older versions as plain `Saved the game`. Chat from players
/// starts with their name in brackets, so it can't pass for one.
fn command_reply(text: &str) -> &str {
    text.strip_prefix("System chat: ").unwrap_or(text)
}

/// Removes leading console-prompt residue (`>`, spaces, carriage returns) before a `[`.
fn strip_prompt(line: &str) -> &str {
    let trimmed = line.trim_start_matches(['>', ' ', '\r']);
    if trimmed.starts_with('[') { trimmed } else { line }
}

fn level_from(text: &str) -> LogLevel {
    match text {
        "TRACE" => LogLevel::Trace,
        "DEBUG" => LogLevel::Debug,
        "INFO" => LogLevel::Info,
        "WARN" | "WARNING" => LogLevel::Warn,
        "ERROR" | "SEVERE" => LogLevel::Error,
        "FATAL" => LogLevel::Fatal,
        _ => LogLevel::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_lines() {
        let line = parse_line(r#"[12:01:22] [Server thread/INFO]: Done (3.421s)! For help, type "help""#);
        assert_eq!(line.level, LogLevel::Info);
        assert_eq!(line.thread.as_deref(), Some("Server thread"));
        assert_eq!(detect_signal(&line.text), Some(GameSignal::Ready));
    }

    #[test]
    fn paper_lines() {
        let line = parse_line(r#"[09:26:31 INFO]: Done (7.015s)! For help, type "help""#);
        assert_eq!(line.level, LogLevel::Info);
        assert_eq!(line.thread, None);
        assert_eq!(detect_signal(&line.text), Some(GameSignal::Ready));
        assert_eq!(
            parse_line("[09:26:31 WARN]: Can't keep up!").level,
            LogLevel::Warn
        );
    }

    /// Regression (real Paper 26.3 run): `JLine` prompt residue in front of log lines.
    #[test]
    fn jline_prompt_prefixes_are_ignored() {
        let line = parse_line(r#">   [07:44:02 INFO]: Done (40.871s)! For help, type "help""#);
        assert_eq!(line.level, LogLevel::Info);
        assert_eq!(detect_signal(&line.text), Some(GameSignal::Ready));
        let doubled = parse_line("> >   [07:47:20 INFO]: There are 0 of a max of 20 players online: ");
        assert_eq!(doubled.text, "There are 0 of a max of 20 players online: ");
        // A line that merely starts with ">" but isn't a log line is kept as is.
        assert_eq!(parse_line("> hello").text, "> hello");
    }

    #[test]
    fn fabric_and_forge_lines() {
        let fabric = parse_line("[12:01:22] [main/INFO] (FabricLoader/GameProvider) Loading Minecraft 26.3");
        assert_eq!(fabric.text, "Loading Minecraft 26.3");
        assert_eq!(fabric.thread.as_deref(), Some("main"));

        let forge = parse_line(
            "[12:01:22] [main/WARN] [cpw.mods.modlauncher.Launcher/MODLAUNCHER]: ModLauncher running",
        );
        assert_eq!(forge.level, LogLevel::Warn);
        assert_eq!(forge.text, "ModLauncher running");
    }

    #[test]
    fn unknown_lines_are_kept_verbatim() {
        let line = parse_line("\tat net.minecraft.server.Main.main(Main.java:42)");
        assert_eq!(line.level, LogLevel::Unknown);
        assert_eq!(line.text, "\tat net.minecraft.server.Main.main(Main.java:42)");
    }

    #[test]
    fn ansi_colours_are_stripped() {
        let line = parse_line("\u{1b}[32m[12:00:00 INFO]: green\u{1b}[0m");
        assert_eq!(line.text, "green");
    }

    #[test]
    fn players_and_saves() {
        assert_eq!(
            detect_signal("Steve joined the game"),
            Some(GameSignal::PlayerJoined { name: "Steve".into() })
        );
        assert_eq!(
            detect_signal("Alex_01 left the game"),
            Some(GameSignal::PlayerLeft {
                name: "Alex_01".into()
            })
        );
        assert_eq!(detect_signal("Saved the game"), Some(GameSignal::Saved));
        assert_eq!(detect_signal("Saving the game (this may take a moment!)"), None);
        assert_eq!(
            detect_signal("<Steve> I joined the game"),
            None,
            "chat can't fake events"
        );
        assert_eq!(detect_signal("Done (abc)!"), None);
    }

    /// Console lines of real 26.3 servers (`NeoForge`, vanilla): replies to commands carry "System chat: ".
    #[test]
    fn saves_are_recognised_in_26_3_command_replies() {
        let lines = [
            "[05:50:32] [Server thread/INFO] [net.minecraft.server.MinecraftServer/]: System chat: Saved the game",
            "[03:12:59] [Server thread/INFO]: System chat: Saved the game",
        ];
        for raw in lines {
            assert_eq!(
                detect_signal(&parse_line(raw).text),
                Some(GameSignal::Saved),
                "{raw}"
            );
        }
        let chat = parse_line("[03:12:59] [Server thread/INFO]: <Steve> System chat: Saved the game");
        assert_eq!(detect_signal(&chat.text), None, "a player can't fake it");
    }

    /// Regression (a real 26.3 server with a real client): joins and leaves are system messages too, and
    /// were missed, so nobody ever seemed to be online there.
    #[test]
    fn joins_and_leaves_are_recognised_in_26_3_system_messages() {
        let join = parse_line("[17:01:05] [Server thread/INFO]: System chat: E2E_Bot joined the game");
        assert_eq!(
            detect_signal(&join.text),
            Some(GameSignal::PlayerJoined {
                name: "E2E_Bot".into()
            })
        );
        let leave = parse_line("[17:01:30] [Server thread/INFO]: System chat: E2E_Bot left the game");
        assert_eq!(
            detect_signal(&leave.text),
            Some(GameSignal::PlayerLeft {
                name: "E2E_Bot".into()
            })
        );
        let chat = parse_line("[17:01:31] [Server thread/INFO]: <Alex> System chat: Notch joined the game");
        assert_eq!(detect_signal(&chat.text), None, "chat can't fake a join");
    }
}
