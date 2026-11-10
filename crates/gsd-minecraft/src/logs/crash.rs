//! The gist of a crash report: what happened, and whose code it was in.
//!
//! Only the start of the file is read: it holds all of this, and the rest can be megabytes. Forge,
//! `NeoForge` and Fabric add a `Suspected Mod` section when they can tell; otherwise the first stack
//! frame outside the game, its loaders and the JVM is the best hint there is, and is shown as just that.

use std::io::{self, Read};

use serde::Serialize;

/// How much of a report is read for its summary.
const MAX_SUMMARY_BYTES: u64 = 256 * 1024;
/// How many stack frames are shown.
const MAX_FRAMES: usize = 6;
/// Whose code a frame belonging to isn't a clue: the JVM, the game, its loaders, and the libraries that
/// ship with them. The first real report named fastutil, which every server has.
const NOT_A_CLUE: [&str; 24] = [
    "it.unimi.dsi.",
    "com.google.",
    "org.apache.",
    "org.slf4j.",
    "org.objectweb.",
    "org.lwjgl.",
    "com.electronwill.",
    "joptsimple.",
    "org.joml.",
    "java.",
    "javax.",
    "jdk.",
    "sun.",
    "net.minecraft.",
    "com.mojang.",
    "cpw.mods.",
    "net.minecraftforge.",
    "net.neoforged.",
    "net.fabricmc.",
    "org.quiltmc.",
    "org.spongepowered.",
    "io.netty.",
    "org.bukkit.",
    "io.papermc.",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CrashSummary {
    /// `Description:` in a game's report; the kind of failure in the JVM's.
    pub description: Option<String>,
    /// When, as the report words it.
    pub time: Option<String>,
    /// The exception, or the JVM's signal.
    pub error: Option<String>,
    /// The first frames of its stack.
    pub frames: Vec<String>,
    /// What the loader itself suspects.
    pub suspected: Vec<String>,
    /// The package of the first frame that isn't the game's, a loader's or the JVM's.
    pub first_outside_code: Option<String>,
}

pub(super) fn summarize_file(file: &mut impl Read) -> io::Result<CrashSummary> {
    let mut bytes = Vec::new();
    file.by_ref().take(MAX_SUMMARY_BYTES).read_to_end(&mut bytes)?;
    Ok(summarize(&String::from_utf8_lossy(&bytes)))
}

/// Summarises a report, whichever kind it is.
pub fn summarize(text: &str) -> CrashSummary {
    if text.contains("# A fatal error has been detected by the Java Runtime Environment")
        || text.contains("# There is insufficient memory for the Java Runtime Environment")
    {
        summarize_jvm(text)
    } else {
        summarize_game(text)
    }
}

/// The game's own format: `Time:`, `Description:`, a blank line, the exception and its frames.
fn summarize_game(text: &str) -> CrashSummary {
    let mut summary = CrashSummary::default();
    let mut lines = text.lines();
    for line in lines.by_ref() {
        if let Some(time) = line.strip_prefix("Time: ") {
            summary.time = Some(time.trim().to_owned());
        } else if let Some(description) = line.strip_prefix("Description: ") {
            summary.description = Some(description.trim().to_owned());
            break;
        }
    }
    let mut rest = lines.skip_while(|line| line.trim().is_empty());
    summary.error = rest.next().map(|line| line.trim().to_owned());
    let all_frames: Vec<&str> = rest
        .take_while(|line| line.trim_start().starts_with("at ") || line.trim_start().starts_with("Caused by"))
        .filter_map(|line| line.trim_start().strip_prefix("at "))
        .collect();
    summary.frames = all_frames
        .iter()
        .take(MAX_FRAMES)
        .map(|frame| (*frame).to_owned())
        .collect();
    summary.first_outside_code = all_frames.iter().find_map(|frame| outside_package(frame));
    summary.suspected = suspected_mods(text);
    summary
}

/// The JVM's format: comment lines at the top say what happened.
fn summarize_jvm(text: &str) -> CrashSummary {
    let comments: Vec<&str> = text
        .lines()
        .take_while(|line| line.starts_with('#') || line.trim().is_empty())
        .map(|line| line.trim_start_matches('#').trim())
        .filter(|line| !line.is_empty())
        .collect();
    let description = comments
        .iter()
        .find(|line| line.starts_with("A fatal error") || line.starts_with("There is insufficient memory"))
        .map(|line| (*line).to_owned());
    // The line after the heading: the signal, or what couldn't be allocated.
    let error = comments
        .iter()
        .position(|line| {
            line.starts_with("A fatal error") || line.starts_with("There is insufficient memory")
        })
        .and_then(|position| comments.get(position + 1))
        .map(|line| (*line).to_owned());
    let frames = comments
        .iter()
        .position(|line| line.starts_with("Problematic frame"))
        .and_then(|position| comments.get(position + 1))
        .map(|line| (*line).to_owned())
        .into_iter()
        .collect();
    let time = text
        .lines()
        .find_map(|line| line.strip_prefix("Time: "))
        .map(|time| time.trim().to_owned());
    CrashSummary {
        description,
        time,
        error,
        frames,
        suspected: Vec::new(),
        first_outside_code: None,
    }
}

/// Forge and `NeoForge`: `Suspected Mods: A (a), Version: 1`. Fabric: `Suspected Mod:` with each below it.
fn suspected_mods(text: &str) -> Vec<String> {
    let mut suspected = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix("Suspected Mods:")
            .or_else(|| trimmed.strip_prefix("Suspected Mod:"))
        else {
            continue;
        };
        let rest = rest.trim();
        if !rest.is_empty() && !rest.eq_ignore_ascii_case("NONE") {
            suspected.push(rest.to_owned());
        }
        // Those listed below it are indented further, and end with the next section.
        let indent = line.len() - line.trim_start().len();
        while let Some(next) = lines.peek() {
            let next_indent = next.len() - next.trim_start().len();
            if next.trim().is_empty() || next_indent <= indent {
                break;
            }
            let mentions_mod = !next.trim_start().starts_with("at ");
            if mentions_mod {
                suspected.push(next.trim().to_owned());
            }
            lines.next();
        }
    }
    suspected.dedup();
    suspected
}

/// `com.example.thing.Class.method(File.java:1)` gives `com.example.thing`, if it isn't known code.
fn outside_package(frame: &str) -> Option<String> {
    // A module prefix: `java.base/java.util…`, or `TRANSFORMER/mod@1.0/com.example…`.
    let call = frame.split('(').next()?.rsplit('/').next()?;
    if NOT_A_CLUE.iter().any(|known| call.starts_with(known)) {
        return None;
    }
    let parts: Vec<&str> = call.split('.').collect();
    // Without the class and the method.
    let package = parts.get(..parts.len().checked_sub(2)?)?;
    (!package.is_empty()).then(|| package.join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the vanilla server writes it when its watchdog stops it.
    const WATCHDOG: &str = "---- Minecraft Crash Report ----
// Who set us up the TNT?

Time: 2026-11-09 12:30:01
Description: Watching Server

java.lang.Error: Watchdog
\tat java.base/java.lang.Thread.sleep0(Native Method)
\tat net.minecraft.server.MinecraftServer.runServer(MinecraftServer.java:771)
\tat java.base/java.lang.Thread.run(Thread.java:1474)


A detailed walkthrough of the error, its code path and all known details is as follows:
---------------------------------------------------------------------------------------
";

    const FORGE: &str = "---- Minecraft Crash Report ----
Time: 2026-11-09 13:00:00
Description: Mod loading error has occurred

java.lang.NullPointerException: Cannot invoke \"Object.hashCode()\"
\tat com.example.coolmod.world.Gen.place(Gen.java:42) ~[coolmod-1.0.jar%23150!/:1.0] {re:classloading}
\tat net.minecraft.world.level.Level.tick(Level.java:1) ~[server.jar:?]
Caused by: java.lang.Error: inner
\tat other.pkg.Inner.run(Inner.java:1)

-- Head --
Thread: Server thread
Suspected Mods: Cool Mod (coolmod), Version: 1.0
\tat com.example.coolmod.world.Gen.place(Gen.java:42)
";

    const FABRIC: &str = "Description: Exception in server tick loop

java.lang.IllegalStateException: nope
\tat a.b.Thing.run(Thing.java:1)

A detailed walkthrough of the error, its code path and all known details is as follows:
-- Head --
\tSuspected Mod:
\t\tNice Mod (nicemod), Version: 2.0
\t\t\tat a.b.Thing.run(Thing.java:1)
\t\tOther Mod (othermod), Version: 3.0
";

    const JVM: &str = "#
# A fatal error has been detected by the Java Runtime Environment:
#
#  SIGSEGV (0xb) at pc=0x00007f, pid=4242, tid=4243
#
# JRE version: OpenJDK Runtime Environment Temurin-25 (25.0+36) (build 25+36-LTS)
# Problematic frame:
# C  [libc.so.6+0xa8b1c]  __pthread_clockjoin_ex+0x13c
#
# Core dump will be written.
#

---------------  S U M M A R Y ------------

Time: Mon Nov  9 12:45:00 2026 UTC elapsed time: 12.3 seconds (0d 0h 0m 12s)
";

    const OUT_OF_MEMORY: &str = "#
# There is insufficient memory for the Java Runtime Environment to continue.
# Native memory allocation (mmap) failed to map 123 bytes. Error detail: committing reserved memory.
#
";

    #[test]
    fn the_watchdogs_report_has_no_clue_outside_the_game() {
        let summary = summarize(WATCHDOG);
        assert_eq!(summary.time.as_deref(), Some("2026-11-09 12:30:01"));
        assert_eq!(summary.description.as_deref(), Some("Watching Server"));
        assert_eq!(summary.error.as_deref(), Some("java.lang.Error: Watchdog"));
        assert_eq!(summary.frames.len(), 3);
        assert_eq!(summary.first_outside_code, None);
        assert_eq!(summary.suspected, Vec::<String>::new());
    }

    #[test]
    fn a_mods_frame_and_the_loaders_suspicion_are_found() {
        let summary = summarize(FORGE);
        assert_eq!(
            summary.first_outside_code.as_deref(),
            Some("com.example.coolmod.world")
        );
        assert_eq!(summary.suspected, ["Cool Mod (coolmod), Version: 1.0"]);
        assert_eq!(summary.frames.len(), 3, "the cause's frames too");

        let summary = summarize(FABRIC);
        assert_eq!(
            summary.suspected,
            [
                "Nice Mod (nicemod), Version: 2.0",
                "Other Mod (othermod), Version: 3.0"
            ]
        );
        assert_eq!(summary.first_outside_code.as_deref(), Some("a.b"));
    }

    #[test]
    fn the_jvms_reports_say_what_happened() {
        let summary = summarize(JVM);
        assert!(
            summary
                .description
                .as_deref()
                .unwrap_or_default()
                .starts_with("A fatal error")
        );
        assert!(
            summary
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with("SIGSEGV")
        );
        assert_eq!(summary.frames.len(), 1);
        assert!(summary.frames[0].contains("libc.so.6"));
        assert!(summary.time.as_deref().unwrap_or_default().contains("2026"));

        let summary = summarize(OUT_OF_MEMORY);
        assert!(
            summary
                .description
                .as_deref()
                .unwrap_or_default()
                .contains("insufficient memory")
        );
        assert!(summary.error.as_deref().unwrap_or_default().contains("mmap"));
    }

    #[test]
    fn anything_else_gives_an_empty_summary_not_an_error() {
        assert_eq!(
            summarize("not a crash report\n"),
            CrashSummary {
                ..CrashSummary::default()
            }
        );
        assert_eq!(summarize(""), CrashSummary::default());
    }

    #[test]
    fn only_a_package_outside_known_code_counts() {
        let cases = [
            ("com.example.Thing.run(Thing.java:1)", Some("com.example")),
            (
                "TRANSFORMER/coolmod@1.0/com.example.Thing.run(Thing.java:1)",
                Some("com.example"),
            ),
            ("java.base/java.util.List.get(List.java:1)", None),
            ("net.minecraft.server.Main.main(Main.java:1)", None),
            ("it.unimi.dsi.fastutil.objects.Thing.get(Thing.java:1)", None),
            ("Thing.run(Thing.java:1)", None),
        ];
        for (frame, package) in cases {
            assert_eq!(outside_package(frame).as_deref(), package, "{frame}");
        }
    }
}
