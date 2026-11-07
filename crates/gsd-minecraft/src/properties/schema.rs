//! What each known `server.properties` key means: type, default, limits, category, help text.
//!
//! The data lives in `server_properties.json` (keys and defaults taken from the file a real 26.3
//! server generates, see D-029). Keys the schema doesn't know (plugins add some) are still
//! shown and editable as plain text.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// Editor sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    General,
    World,
    Players,
    Performance,
    Network,
    ResourcePack,
    RemoteControl,
    Advanced,
}

/// What values a key accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValueKind {
    Bool,
    Int {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    Choice {
        options: Vec<String>,
    },
    /// Free text with suggested values (mods add world types and data packs).
    Suggest {
        options: Vec<String>,
    },
    Text,
}

/// One known key.
// Not `deny_unknown_fields` (serde can't combine it with `flatten`): `schema_json_uses_only_known_fields` checks
// the JSON's field names instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertySchema {
    pub key: String,
    pub label: String,
    pub description: String,
    pub category: Category,
    #[serde(flatten)]
    pub kind: ValueKind,
    /// The value a freshly generated file has.
    pub default: String,
    /// Shown only when the user asks for advanced settings.
    #[serde(default)]
    pub advanced: bool,
    /// Hidden from users who can't edit properties (passwords, secrets).
    #[serde(default)]
    pub sensitive: bool,
    /// Set by the app (ports the sandbox allows); shown read-only.
    #[serde(default)]
    pub managed: bool,
}

static SCHEMA: LazyLock<Vec<PropertySchema>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("server_properties.json"))
        // WHY: the file is embedded at compile time and `schema_is_valid` checks it in tests.
        .unwrap_or_else(|error| unreachable!("embedded server_properties.json is invalid: {error}"))
});

/// Every known key, in editor order.
pub fn all() -> &'static [PropertySchema] {
    &SCHEMA
}

pub fn find(key: &str) -> Option<&'static PropertySchema> {
    SCHEMA.iter().find(|schema| schema.key == key)
}

impl PropertySchema {
    /// `Err` with a message for the user when `value` isn't acceptable for this key.
    pub fn validate(&self, value: &str) -> Result<(), String> {
        match &self.kind {
            ValueKind::Bool if matches!(value, "true" | "false") => Ok(()),
            ValueKind::Bool => Err("must be true or false".into()),
            ValueKind::Int { min, max } => validate_int(value, *min, *max),
            ValueKind::Choice { options } if options.iter().any(|o| o == value) => Ok(()),
            ValueKind::Choice { options } => Err(format!("must be one of: {}", options.join(", "))),
            ValueKind::Suggest { .. } | ValueKind::Text => Ok(()),
        }
    }
}

fn validate_int(value: &str, min: Option<i64>, max: Option<i64>) -> Result<(), String> {
    let number: i64 = value
        .trim()
        .parse()
        .map_err(|_| "must be a whole number".to_owned())?;
    match (min, max) {
        (Some(min), Some(max)) if !(min..=max).contains(&number) => {
            Err(format!("must be between {min} and {max}"))
        }
        (Some(min), None) if number < min => Err(format!("must be at least {min}")),
        (None, Some(max)) if number > max => Err(format!("must be at most {max}")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn schema_json_uses_only_known_fields() {
        let known = [
            "key",
            "label",
            "description",
            "category",
            "type",
            "min",
            "max",
            "options",
            "default",
            "advanced",
            "sensitive",
            "managed",
        ];
        let entries: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(include_str!("server_properties.json")).unwrap();
        for entry in entries {
            for field in entry.keys() {
                assert!(
                    known.contains(&field.as_str()),
                    "unknown field {field} in {:?}",
                    entry.get("key")
                );
            }
        }
    }

    #[test]
    fn schema_is_valid() {
        let schema = all();
        assert!(schema.len() > 40, "{}", schema.len());
        let mut keys = HashSet::new();
        for entry in schema {
            assert!(keys.insert(entry.key.as_str()), "duplicate key {}", entry.key);
            assert!(
                !entry.label.is_empty() && !entry.description.is_empty(),
                "{}",
                entry.key
            );
            assert_eq!(entry.validate(&entry.default), Ok(()), "default of {}", entry.key);
            if let ValueKind::Int {
                min: Some(min),
                max: Some(max),
            } = entry.kind
            {
                assert!(min < max, "{}", entry.key);
            }
        }
        for managed in ["server-port", "query.port", "rcon.port", "management-server-port"] {
            assert!(
                find(managed).is_some_and(|s| s.managed),
                "{managed} must be managed"
            );
        }
        assert!(find("rcon.password").is_some_and(|s| s.sensitive));
        assert!(find("management-server-secret").is_some_and(|s| s.sensitive));
    }

    #[test]
    fn validates_by_type() {
        let int = |min, max| PropertySchema {
            key: "k".into(),
            label: "K".into(),
            description: "d".into(),
            category: Category::General,
            kind: ValueKind::Int { min, max },
            default: "1".into(),
            advanced: false,
            sensitive: false,
            managed: false,
        };
        assert!(int(Some(1), Some(10)).validate("5").is_ok());
        assert_eq!(
            int(Some(1), Some(10)).validate("11"),
            Err("must be between 1 and 10".into())
        );
        assert_eq!(
            int(Some(0), None).validate("-1"),
            Err("must be at least 0".into())
        );
        assert_eq!(int(None, Some(3)).validate("4"), Err("must be at most 3".into()));
        assert_eq!(
            int(None, None).validate("ten"),
            Err("must be a whole number".into())
        );
        assert!(int(None, None).validate("-2147483648").is_ok());

        let difficulty = find("difficulty").unwrap();
        assert!(difficulty.validate("hard").is_ok());
        assert!(difficulty.validate("Hard").unwrap_err().contains("peaceful"));
        let pvp_like = find("hardcore").unwrap();
        assert!(pvp_like.validate("true").is_ok());
        assert!(pvp_like.validate("yes").is_err());
        assert!(find("motd").unwrap().validate("anything at all").is_ok());
        assert!(find("not-a-real-key").is_none());
    }
}
