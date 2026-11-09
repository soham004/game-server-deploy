//! Known failure patterns, explained in plain language.

use std::sync::LazyLock;

use gsd_core::game::{Diagnostic, Severity};
use regex::Regex;

struct Rule {
    pattern: Regex,
    code: &'static str,
    severity: Severity,
    title: &'static str,
    detail: fn(&regex::Captures<'_>) -> String,
}

fn rule(
    pattern: &str,
    code: &'static str,
    severity: Severity,
    title: &'static str,
    detail: fn(&regex::Captures<'_>) -> String,
) -> Rule {
    Rule {
        pattern: Regex::new(pattern).unwrap_or_else(|e| unreachable!("static regex: {e}")),
        code,
        severity,
        title,
        detail,
    }
}

/// Every port-binding failure shares one code: Minecraft logs "FAILED TO BIND TO PORT!" and then
/// the cause on the next line, so the specific explanation replaces the generic one (the actor
/// keeps one diagnostic per code).
const PORT_BIND_FAILED: &str = "port_bind_failed";

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(
            r"FAILED TO BIND TO PORT",
            PORT_BIND_FAILED,
            Severity::Error,
            "Couldn't open the server port",
            |_| {
                "The server could not listen on its network port. The console lines after this one say why."
                    .into()
            },
        ),
        rule(
            r"(?i)bind\(\.\.\) failed with error\(-98\)|BindException: Address already in use",
            PORT_BIND_FAILED,
            Severity::Error,
            "Port already in use",
            |_| {
                "Another program (or another server) is using this server's port. Stop it or change the port in the server settings.".into()
            },
        ),
        rule(
            r"bind\(\.\.\) failed with error\(-22\)",
            PORT_BIND_FAILED,
            Severity::Error,
            "The sandbox rejected the server's port setup",
            |_| {
                "This happens when Java runs with -Djava.net.preferIPv6Addresses=system. Remove that flag from the server's JVM arguments.".into()
            },
        ),
        rule(
            r"(?i)bind\(\.\.\) failed with error\(-13\)|BindException: Permission denied",
            PORT_BIND_FAILED,
            Severity::Error,
            "Port not allowed",
            |_| {
                "Each server may only listen on the port it was given. Check that server-port (and query/RCON ports) in server.properties weren't changed by hand.".into()
            },
        ),
        rule(
            r"You need to agree to the EULA",
            "eula_not_accepted",
            Severity::Error,
            "EULA not accepted",
            |_| "The Minecraft EULA must be accepted before the server can start.".into(),
        ),
        rule(
            r"class file version (\d+)\.\d+",
            "wrong_java",
            Severity::Error,
            "Wrong Java version",
            |captures| {
                let needed = captures
                    .get(1)
                    .and_then(|m| m.as_str().parse::<u32>().ok())
                    .map(|v| v.saturating_sub(44));
                match needed {
                    Some(major) => format!(
                        "This server (or a mod) needs Java {major} or newer. Pick a matching Java runtime in the server settings."
                    ),
                    None => "This server needs a newer Java version.".into(),
                }
            },
        ),
        rule(
            r"UnsupportedClassVersionError",
            "wrong_java",
            Severity::Error,
            "Wrong Java version",
            |_| {
                "This server (or a mod) needs a newer Java version. Pick a matching Java runtime in the server settings.".into()
            },
        ),
        rule(
            r"java\.net\.UnknownHostException: (\S+)",
            "network_unreachable",
            Severity::Error,
            "Can't reach the internet",
            |captures| {
                let host = captures.get(1).map_or("a download server", |m| m.as_str());
                format!(
                    "The server could not look up {host}. Check that this machine has a working internet connection and DNS."
                )
            },
        ),
        rule(
            r"java\.lang\.OutOfMemoryError",
            "out_of_memory",
            Severity::Error,
            "Out of memory",
            |_| {
                "The server ran out of memory. Increase its maximum memory in the server settings, or remove heavy mods.".into()
            },
        ),
        rule(
            r"Can't keep up! Is the server overloaded\?",
            "overloaded",
            Severity::Warning,
            "Server can't keep up",
            |_| {
                "The server is falling behind (lag). Lower the view/simulation distance, pre-generate the world, or give it more CPU.".into()
            },
        ),
        rule(
            r"(?i)(Incompatible mods? found|requires .* which is missing|Missing or unsupported mandatory dependencies)",
            "missing_dependency",
            Severity::Error,
            "A mod is missing a dependency",
            |_| {
                "A mod needs another mod (or a different version of one) that isn't installed. The lines above name it.".into()
            },
        ),
        rule(
            r"(Environment type CLIENT|NoClassDefFoundError: net/minecraft/client)",
            "client_only_mod",
            Severity::Error,
            "Client-only mod installed",
            |_| {
                "A mod that only works in the game client is installed on the server. Remove it from the mods folder.".into()
            },
        ),
        rule(
            r"(Mixin apply failed|MixinApplyError|Mixin transformation of .* failed)",
            "mixin_failure",
            Severity::Error,
            "A mod failed to load",
            |_| {
                "A mod couldn't patch the game (mixin failure), often because of an incompatible mod or game version.".into()
            },
        ),
        rule(
            r"session\.lock: already locked",
            "world_locked",
            Severity::Error,
            "World in use by another process",
            |_| {
                "Another Java process still has this world open. A server can take a moment to save after the app was killed, so try again shortly. If this keeps happening, stop that process on the host.".into()
            },
        ),
    ]
});

/// The first rule matching `raw`, if any.
pub fn diagnose(raw: &str) -> Option<Diagnostic> {
    RULES.iter().find_map(|rule| {
        rule.pattern.captures(raw).map(|captures| Diagnostic {
            code: rule.code,
            severity: rule.severity,
            title: rule.title.into(),
            detail: (rule.detail)(&captures),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact lines from the real Forge 26.3 run (2026-11-07), plus the other causes.
    #[test]
    fn explains_why_binding_the_port_failed() {
        let cases = [
            (
                "[07Nov2026 15:29:49.020] [Server thread/WARN] [net.minecraft.server.dedicated.DedicatedServer/]: **** FAILED TO BIND TO PORT!",
                "Couldn't open the server port",
            ),
            (
                "[07Nov2026 15:29:49.021] [Server thread/WARN] [net.minecraft.server.dedicated.DedicatedServer/]: The exception was: io.netty.channel.unix.Errors$NativeIoException: bind(..) failed with error(-22): Invalid argument",
                "The sandbox rejected the server's port setup",
            ),
            (
                "[12:00:00] [Server thread/WARN]: The exception was: io.netty.channel.unix.Errors$NativeIoException: bind(..) failed with error(-98): Address already in use",
                "Port already in use",
            ),
            (
                "[12:00:00] [Server thread/WARN]: The exception was: java.net.BindException: Address already in use",
                "Port already in use",
            ),
            (
                "[12:00:00] [Server thread/WARN]: The exception was: io.netty.channel.unix.Errors$NativeIoException: bind(..) failed with error(-13): Permission denied",
                "Port not allowed",
            ),
        ];
        for (line, title) in cases {
            let diagnostic = diagnose(line).unwrap_or_else(|| panic!("no diagnosis for {line}"));
            assert_eq!(
                (diagnostic.code, diagnostic.title.as_str()),
                (PORT_BIND_FAILED, title),
                "{line}"
            );
        }
        assert!(
            diagnose("[12:00:00] [Server thread/WARN]: Perhaps a server is already running on that port?")
                .is_none()
        );
    }

    #[test]
    fn recognises_common_failures() {
        let cases = [
            (
                "[12:00:00] [main/INFO]: You need to agree to the EULA in order to run the server.",
                "eula_not_accepted",
            ),
            (
                "Exception in thread \"main\" java.lang.OutOfMemoryError: Java heap space",
                "out_of_memory",
            ),
            (
                "[12:00:00 WARN]: Can't keep up! Is the server overloaded? Running 2500ms behind",
                "overloaded",
            ),
            (
                "java.net.UnknownHostException: piston-data.mojang.com",
                "network_unreachable",
            ),
            ("Incompatible mods found!", "missing_dependency"),
            (
                "Mod 'X' requires mod 'fabric-api' which is missing!",
                "missing_dependency",
            ),
            (
                "java.lang.NoClassDefFoundError: net/minecraft/client/MinecraftClient",
                "client_only_mod",
            ),
            (
                "org.spongepowered.asm.mixin.transformer.throwables.MixinApplyError: Mixin apply failed",
                "mixin_failure",
            ),
            (
                "net.minecraft.util.DirectoryLock$LockException: ./world/session.lock: already locked (possibly by other Minecraft instance?)",
                "world_locked",
            ),
        ];
        for (line, code) in cases {
            assert_eq!(diagnose(line).map(|d| d.code), Some(code), "{line}");
        }
        assert!(diagnose("[12:00:00] [Server thread/INFO]: Steve joined the game").is_none());
    }

    #[test]
    fn wrong_java_names_the_needed_version() {
        let line = "java.lang.UnsupportedClassVersionError: net/minecraft/bundler/Main has been compiled by a more recent version of the Java Runtime (class file version 69.0), this version of the Java Runtime only recognizes class file versions up to 65.0";
        let diagnostic = diagnose(line).unwrap();
        assert_eq!(diagnostic.code, "wrong_java");
        assert!(diagnostic.detail.contains("Java 25"), "{}", diagnostic.detail);
    }
}
