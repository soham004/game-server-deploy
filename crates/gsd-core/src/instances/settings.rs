//! Changing an instance's settings: name, memory, JVM tuning, Java runtime and behaviour.
//!
//! [`SettingsChange::apply`] is pure validation plus assignment, so every rule is unit-tested;
//! `InstanceService::update_settings` adds the checks that need the outside world (whether a
//! chosen Java runtime exists and may be used).

use serde::Deserialize;

use super::definition::{InstanceDefinition, JvmPreset, RuntimeChoice};

pub const MAX_NAME_CHARS: usize = 64;
pub const MIN_MEMORY_MB: u32 = 512;
pub const MAX_MEMORY_MB: u32 = 64 * 1024;
const MAX_JVM_ARGS: usize = 64;
const MAX_JVM_ARG_LEN: usize = 512;
const MAX_STOP_TIMEOUT_SECS: u64 = 15 * 60;

/// JVM options an instance may not add (exactly these), and why.
const FORBIDDEN_JVM_ARGS: &[(&str, &str)] = &[
    ("-jar", "the launch command is managed by the app"),
    ("-cp", "the launch command is managed by the app"),
    ("-classpath", "the launch command is managed by the app"),
    ("--class-path", "the launch command is managed by the app"),
    (
        "-Djava.net.preferIPv6Addresses=system",
        "it stops the server from opening its port inside the sandbox (D-028)",
    ),
];

/// JVM options an instance may not add (anything starting with these), and why.
const FORBIDDEN_JVM_ARG_PREFIXES: &[(&str, &str)] = &[
    ("-Xmx", "set the maximum memory with the memory setting"),
    ("-Xms", "set the starting memory with the memory setting"),
];

/// What to change; absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChange {
    pub name: Option<String>,
    pub min_memory_mb: Option<u32>,
    pub max_memory_mb: Option<u32>,
    pub jvm_preset: Option<JvmPreset>,
    pub extra_jvm_args: Option<Vec<String>>,
    pub java_runtime: Option<RuntimeChoice>,
    pub auto_start: Option<bool>,
    pub auto_restart: Option<bool>,
    pub crash_limit: Option<u32>,
    pub stop_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("the name must be 1 to {MAX_NAME_CHARS} characters")]
    InvalidName,
    #[error(
        "memory must be between {MIN_MEMORY_MB} and {MAX_MEMORY_MB} MB, and the minimum at most the maximum"
    )]
    InvalidMemory,
    #[error("JVM argument {arg:?} is not allowed: {reason}")]
    ForbiddenJvmArg { arg: String, reason: String },
    #[error("at most {MAX_JVM_ARGS} JVM arguments")]
    TooManyJvmArgs,
    #[error("crash limit must be 1 to 20")]
    InvalidCrashLimit,
    #[error("stop timeout must be 5 to {MAX_STOP_TIMEOUT_SECS} seconds")]
    InvalidStopTimeout,
}

impl SettingsChange {
    /// Validates everything first, then changes `definition`; on error nothing changes.
    pub fn apply(&self, definition: &mut InstanceDefinition) -> Result<(), SettingsError> {
        let mut next = definition.clone();
        if let Some(name) = &self.name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                return Err(SettingsError::InvalidName);
            }
            name.clone_into(&mut next.name);
        }
        self.apply_java(&mut next)?;
        self.apply_behavior(&mut next)?;
        *definition = next;
        Ok(())
    }

    fn apply_java(&self, next: &mut InstanceDefinition) -> Result<(), SettingsError> {
        let java = &mut next.java;
        java.min_memory_mb = self.min_memory_mb.unwrap_or(java.min_memory_mb);
        java.max_memory_mb = self.max_memory_mb.unwrap_or(java.max_memory_mb);
        let memory_range = MIN_MEMORY_MB..=MAX_MEMORY_MB;
        if !memory_range.contains(&java.min_memory_mb)
            || !memory_range.contains(&java.max_memory_mb)
            || java.min_memory_mb > java.max_memory_mb
        {
            return Err(SettingsError::InvalidMemory);
        }
        if let Some(preset) = self.jvm_preset {
            java.preset = preset;
        }
        if let Some(args) = &self.extra_jvm_args {
            java.extra_jvm_args = validate_jvm_args(args)?;
        }
        if let Some(runtime) = &self.java_runtime {
            java.runtime = runtime.clone();
        }
        Ok(())
    }

    fn apply_behavior(&self, next: &mut InstanceDefinition) -> Result<(), SettingsError> {
        let behavior = &mut next.behavior;
        behavior.auto_start = self.auto_start.unwrap_or(behavior.auto_start);
        behavior.auto_restart = self.auto_restart.unwrap_or(behavior.auto_restart);
        if let Some(limit) = self.crash_limit {
            if !(1..=20).contains(&limit) {
                return Err(SettingsError::InvalidCrashLimit);
            }
            behavior.crash_limit = limit;
        }
        if let Some(timeout) = self.stop_timeout_secs {
            if !(5..=MAX_STOP_TIMEOUT_SECS).contains(&timeout) {
                return Err(SettingsError::InvalidStopTimeout);
            }
            behavior.stop_timeout_secs = timeout;
        }
        Ok(())
    }
}

/// Trims, drops empty entries, and checks each argument is a JVM option the launch can carry.
fn validate_jvm_args(args: &[String]) -> Result<Vec<String>, SettingsError> {
    let args: Vec<String> = args
        .iter()
        .map(|arg| arg.trim().to_owned())
        .filter(|arg| !arg.is_empty())
        .collect();
    if args.len() > MAX_JVM_ARGS {
        return Err(SettingsError::TooManyJvmArgs);
    }
    for arg in &args {
        let forbidden = |reason: &str| SettingsError::ForbiddenJvmArg {
            arg: arg.clone(),
            reason: reason.to_owned(),
        };
        if !arg.starts_with('-') {
            return Err(forbidden("JVM options start with '-'"));
        }
        if arg.len() > MAX_JVM_ARG_LEN || arg.chars().any(char::is_control) {
            return Err(forbidden("too long or contains control characters"));
        }
        let rule = FORBIDDEN_JVM_ARGS
            .iter()
            .find(|(forbidden_arg, _)| arg == forbidden_arg)
            .or_else(|| {
                FORBIDDEN_JVM_ARG_PREFIXES
                    .iter()
                    .find(|(prefix, _)| arg.starts_with(prefix))
            });
        if let Some((_, reason)) = rule {
            return Err(forbidden(reason));
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::definition::tests::sample;

    fn applied(change: SettingsChange) -> Result<InstanceDefinition, SettingsError> {
        applied_to(&sample(), change)
    }

    fn applied_to(
        before: &InstanceDefinition,
        change: SettingsChange,
    ) -> Result<InstanceDefinition, SettingsError> {
        let mut definition = before.clone();
        change.apply(&mut definition).map(|()| definition)
    }

    #[test]
    fn applies_only_what_is_given() {
        let before = sample();
        let after = applied_to(
            &before,
            SettingsChange {
                name: Some("  Creative  ".into()),
                max_memory_mb: Some(6144),
                auto_start: Some(true),
                ..SettingsChange::default()
            },
        )
        .unwrap();
        assert_eq!(after.name, "Creative");
        assert_eq!(after.java.max_memory_mb, 6144);
        assert_eq!(after.java.min_memory_mb, before.java.min_memory_mb);
        assert!(after.behavior.auto_start);
        assert_eq!(after.behavior.crash_limit, before.behavior.crash_limit);
        assert_eq!(applied_to(&before, SettingsChange::default()).unwrap(), before);
    }

    #[test]
    fn rejects_bad_values_and_changes_nothing() {
        let cases = [
            (
                SettingsChange {
                    name: Some("   ".into()),
                    ..Default::default()
                },
                SettingsError::InvalidName,
            ),
            (
                SettingsChange {
                    name: Some("x".repeat(65)),
                    ..Default::default()
                },
                SettingsError::InvalidName,
            ),
            (
                SettingsChange {
                    max_memory_mb: Some(256),
                    ..Default::default()
                },
                SettingsError::InvalidMemory,
            ),
            (
                SettingsChange {
                    max_memory_mb: Some(MAX_MEMORY_MB + 1),
                    ..Default::default()
                },
                SettingsError::InvalidMemory,
            ),
            (
                SettingsChange {
                    min_memory_mb: Some(4096),
                    max_memory_mb: Some(2048),
                    ..Default::default()
                },
                SettingsError::InvalidMemory,
            ),
            (
                SettingsChange {
                    crash_limit: Some(0),
                    ..Default::default()
                },
                SettingsError::InvalidCrashLimit,
            ),
            (
                SettingsChange {
                    stop_timeout_secs: Some(1),
                    ..Default::default()
                },
                SettingsError::InvalidStopTimeout,
            ),
        ];
        for (change, expected) in cases {
            let original = sample();
            let mut definition = original.clone();
            assert_eq!(change.apply(&mut definition), Err(expected), "{change:?}");
            assert_eq!(definition, original, "nothing changed for {change:?}");
        }
        // An invalid field fails the whole change, even after valid ones.
        let mut definition = sample();
        let mixed = SettingsChange {
            name: Some("Ok".into()),
            crash_limit: Some(99),
            ..Default::default()
        };
        assert!(mixed.apply(&mut definition).is_err());
        assert_eq!(definition.name, sample().name);
    }

    #[test]
    fn jvm_args_are_cleaned_and_checked() {
        let ok = applied(SettingsChange {
            extra_jvm_args: Some(vec![
                "  -XX:+UseZGC ".into(),
                String::new(),
                "-Dfoo=bar".into(),
                "-Xss2M".into(),
            ]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(ok.java.extra_jvm_args, ["-XX:+UseZGC", "-Dfoo=bar", "-Xss2M"]);

        for bad in [
            "-Xmx8G",
            "-Xms1G",
            "-jar",
            "-cp",
            "-classpath",
            "@args.txt",
            "-Djava.net.preferIPv6Addresses=system",
            "not-an-option",
            "-Dnew=line\nbreak",
        ] {
            let result = applied(SettingsChange {
                extra_jvm_args: Some(vec![bad.into()]),
                ..Default::default()
            });
            assert!(
                matches!(result, Err(SettingsError::ForbiddenJvmArg { .. })),
                "{bad:?}: {result:?}"
            );
        }
        let many = applied(SettingsChange {
            extra_jvm_args: Some(vec!["-Da=b".into(); 65]),
            ..Default::default()
        });
        assert_eq!(many.unwrap_err(), SettingsError::TooManyJvmArgs);
    }

    #[test]
    fn runtime_and_preset_are_set() {
        let after = applied(SettingsChange {
            jvm_preset: Some(JvmPreset::None),
            java_runtime: Some(RuntimeChoice::Runtime {
                id: "temurin-21-x64".into(),
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(after.java.preset, JvmPreset::None);
        assert_eq!(
            after.java.runtime,
            RuntimeChoice::Runtime {
                id: "temurin-21-x64".into()
            }
        );
    }
}
