//! Strongly typed identifiers.
//!
//! Every entity gets its own newtype around a ULID so a `UserId` can never be passed where an
//! `InstanceId` is expected. ULIDs sort by creation time, which keeps database indexes tidy.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Returned when a string is not a valid identifier.
#[derive(Debug, thiserror::Error)]
#[error("invalid identifier: {0:?}")]
pub struct IdParseError(String);

macro_rules! ulid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            /// Creates a new, unique identifier.
            pub fn generate() -> Self {
                Self(Ulid::generate())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(raw).map(Self).map_err(|_| IdParseError(raw.to_owned()))
            }
        }
    };
}

ulid_id!(
    /// Identifies a user account.
    UserId
);
ulid_id!(
    /// Identifies a server instance.
    InstanceId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_strings() {
        let id = UserId::generate();
        let parsed: UserId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn rejects_garbage() {
        assert!("not-an-id".parse::<InstanceId>().is_err());
    }
}
