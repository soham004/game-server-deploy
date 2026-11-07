//! Ordering loader version strings ("0.24.0", "0.20.0-beta.9", "26.3.0.64-beta", "66.1.5").
//!
//! Upstreams don't always list versions in order (Quilt's meta doesn't), and strings don't sort
//! ("0.9" > "0.10"). Rules, close to semver: numeric dot-separated parts compare as numbers
//! (missing parts count as 0); a pre-release (`-…`) sorts before its release; build metadata
//! (`+…`) only breaks ties. Not for Minecraft versions themselves: those sort by release time.

use std::cmp::Ordering;

/// Compares two loader versions (`a < b` means `a` is older).
pub fn compare(a: &str, b: &str) -> Ordering {
    let (a_core, a_build) = split_once_or_all(a, '+');
    let (b_core, b_build) = split_once_or_all(b, '+');
    let (a_release, a_pre) = split_once_or_all(a_core, '-');
    let (b_release, b_pre) = split_once_or_all(b_core, '-');

    compare_dotted(a_release, b_release)
        .then_with(|| compare_pre_release(a_pre, b_pre))
        .then_with(|| compare_dotted(a_build, b_build))
}

/// Whether `version` is a pre-release or snapshot build (`-beta`, `+snapshot-3`, …).
pub fn is_pre_release(version: &str) -> bool {
    version.contains(['-', '+'])
}

/// Newest first.
pub fn sort_newest_first<T>(items: &mut [T], version: impl Fn(&T) -> &str) {
    items.sort_by(|a, b| compare(version(b), version(a)));
}

fn split_once_or_all(text: &str, separator: char) -> (&str, &str) {
    text.split_once(separator).unwrap_or((text, ""))
}

/// No pre-release ranks above any pre-release of the same release.
fn compare_pre_release(a: &str, b: &str) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => compare_dotted(a, b),
    }
}

/// Part by part: numbers numerically, text alphabetically, a number before text.
fn compare_dotted(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.').filter(|p| !p.is_empty());
    let mut b_parts = b.split('.').filter(|p| !p.is_empty());
    loop {
        let ordering = match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (Some(part), None) => compare_part(part, "0"),
            (None, Some(part)) => compare_part("0", part),
            (Some(a), Some(b)) => compare_part(a, b),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

fn compare_part(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn assert_ascending(versions: &[&str]) {
        for pair in versions.windows(2) {
            assert_eq!(
                compare(pair[0], pair[1]),
                Ordering::Less,
                "{} < {}",
                pair[0],
                pair[1]
            );
            assert_eq!(
                compare(pair[1], pair[0]),
                Ordering::Greater,
                "{} > {}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn numbers_compare_numerically() {
        assert_ascending(&["0.9", "0.10", "0.24.0", "1.0"]);
        assert_ascending(&["66.1.0", "66.1.5", "66.1.10"]);
        assert_ascending(&["26.3.0.99", "26.3.0.100", "26.3.0.101"]);
    }

    #[test]
    fn pre_releases_sort_before_their_release() {
        // Real Quilt and NeoForge versions.
        assert_ascending(&[
            "0.20.0-beta.1",
            "0.20.0-beta.2",
            "0.20.0-beta.9",
            "0.20.0-beta.10",
            "0.20.0",
            "0.20.2-beta.1",
            "0.24.0",
        ]);
        assert_ascending(&["26.3.0.75-beta", "26.3.0.76", "26.3.0.101"]);
    }

    #[test]
    fn missing_parts_are_zero_and_build_metadata_breaks_ties() {
        assert_eq!(compare("1.0", "1.0.0"), Ordering::Equal);
        assert_ascending(&["1.0+build.1", "1.0+build.2"]);
        assert_eq!(compare("1.2.3", "1.2.3"), Ordering::Equal);
    }

    #[test]
    fn text_parts_never_panic_and_sort_after_numbers() {
        assert_ascending(&["1.0", "1.x"]);
        assert_ascending(&["0.25w14craftmine.3-beta", "0.25w14craftmine.4-beta"]);
        assert_eq!(compare("", ""), Ordering::Equal);
    }

    #[test]
    fn sorts_a_shuffled_list_newest_first() {
        // The order Quilt's meta returned for 26.3 (2026-11-07).
        let mut versions = vec![
            "0.20.0-beta.9",
            "0.20.0-beta.7",
            "0.20.0-beta.8",
            "0.20.0-beta.1",
            "0.24.0",
            "0.20.2-beta.1",
        ];
        sort_newest_first(&mut versions, |v| v);
        assert_eq!(
            versions,
            [
                "0.24.0",
                "0.20.2-beta.1",
                "0.20.0-beta.9",
                "0.20.0-beta.8",
                "0.20.0-beta.7",
                "0.20.0-beta.1"
            ]
        );
    }

    #[test]
    fn detects_pre_releases() {
        assert!(is_pre_release("0.20.0-beta.9"));
        assert!(is_pre_release("26.4.0.0-alpha.1+snapshot-3"));
        assert!(!is_pre_release("66.1.0"));
    }
}
