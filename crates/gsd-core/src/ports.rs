//! Port ranges from which game ports and internal (RCON / management) ports are allocated.
//!
//! In Docker, published ports must be declared up front, so instances only ever receive ports
//! from a configured range (see `claude/decisions.md` D-022).

use std::{fmt, ops::RangeInclusive, str::FromStr};

use serde::{Deserialize, Serialize};

/// Why a port range string was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortRangeError {
    #[error("expected a port or a range like 25565-25584, got {0:?}")]
    Malformed(String),
    #[error("ports must be between 1 and 65535 and the range must not be reversed, got {0:?}")]
    OutOfOrder(String),
}

/// An inclusive range of TCP/UDP ports, e.g. `25565-25584`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PortRange {
    first: u16,
    last: u16,
}

impl PortRange {
    /// Creates a range; `first` must be non-zero and not greater than `last`.
    pub fn new(first: u16, last: u16) -> Result<Self, PortRangeError> {
        if first == 0 || first > last {
            return Err(PortRangeError::OutOfOrder(format!("{first}-{last}")));
        }
        Ok(Self { first, last })
    }

    pub fn first(self) -> u16 {
        self.first
    }

    pub fn last(self) -> u16 {
        self.last
    }

    /// Number of ports in the range (always at least one).
    pub fn count(self) -> usize {
        usize::from(self.last - self.first) + 1
    }

    pub fn contains(self, port: u16) -> bool {
        (self.first..=self.last).contains(&port)
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.first <= other.last && other.first <= self.last
    }

    pub fn ports(self) -> RangeInclusive<u16> {
        self.first..=self.last
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.first, self.last)
    }
}

impl FromStr for PortRange {
    type Err = PortRangeError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let malformed = || PortRangeError::Malformed(raw.to_owned());
        let parse = |part: &str| part.trim().parse::<u16>().map_err(|_| malformed());

        if let Some((first, last)) = raw.split_once('-') {
            return Self::new(parse(first)?, parse(last)?);
        }
        let port = parse(raw)?;
        Self::new(port, port)
    }
}

impl TryFrom<String> for PortRange {
    type Error = PortRangeError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        raw.parse()
    }
}

impl From<PortRange> for String {
    fn from(range: PortRange) -> Self {
        range.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges_and_single_ports() {
        let range: PortRange = "25565-25584".parse().unwrap();
        assert_eq!((range.first(), range.last(), range.count()), (25565, 25584, 20));

        let single: PortRange = "25565".parse().unwrap();
        assert_eq!(single.count(), 1);
    }

    #[test]
    fn rejects_bad_ranges() {
        assert!("abc".parse::<PortRange>().is_err());
        assert!("200-100".parse::<PortRange>().is_err());
        assert!("0-10".parse::<PortRange>().is_err());
        assert!("1-70000".parse::<PortRange>().is_err());
    }

    #[test]
    fn detects_overlap() {
        let game: PortRange = "25565-25584".parse().unwrap();
        let internal: PortRange = "35565-35664".parse().unwrap();
        let clash: PortRange = "25580-25600".parse().unwrap();
        assert!(!game.overlaps(internal));
        assert!(game.overlaps(clash));
    }
}
