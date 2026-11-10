//! Player names and UUIDs.

use md5::{Digest, Md5};

/// The longest name Minecraft allows.
pub const MAX_NAME_CHARS: usize = 16;

/// A name an account can have: letters, digits and `_`, at most 16. Offline servers accept more, but
/// those names can't be told apart from what else a command takes (`@a` is everyone), so the app only
/// manages these.
pub fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_CHARS).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The UUID an offline-mode server gives a name: Java's `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`,
/// an MD5-based (version 3) UUID.
pub fn offline_uuid(name: &str) -> String {
    let mut hash: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    hash[6] = (hash[6] & 0x0f) | 0x30;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    hyphenated(&hex::encode(hash))
}

/// A UUID in its usual form (lowercase, with hyphens), from either form; `None` if it isn't one.
pub fn normalize_uuid(text: &str) -> Option<String> {
    let plain: String = text.chars().filter(|c| *c != '-').collect();
    let well_formed = plain.len() == 32
        && plain.chars().all(|c| c.is_ascii_hexdigit())
        && (text.len() == 32 || is_hyphenated(text));
    well_formed.then(|| hyphenated(&plain.to_ascii_lowercase()))
}

/// Offline-mode UUIDs are version 3; accounts have version 4. Only accounts have skins.
pub fn is_account_uuid(uuid: &str) -> bool {
    uuid.as_bytes().get(14) == Some(&b'4')
}

fn is_hyphenated(text: &str) -> bool {
    text.len() == 36 && [8, 13, 18, 23].iter().all(|&at| text.as_bytes()[at] == b'-')
}

fn hyphenated(hex: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_an_account_could_have() {
        for good in ["Steve", "E2E_Bot", "a", "abcdefghijklmnop"] {
            assert!(is_valid_name(good), "{good}");
        }
        for bad in [
            "",
            "abcdefghijklmnopq",
            "@a",
            "two words",
            "x\ny",
            "dé",
            "Steve;stop",
        ] {
            assert!(!is_valid_name(bad), "{bad:?}");
        }
    }

    /// The values a real 26.3 server in offline mode gave these names.
    #[test]
    fn offline_uuids_match_the_servers_own() {
        assert_eq!(offline_uuid("E2E_Bot"), "75facf7f-9320-3e29-b801-7bae224ea921");
        assert_eq!(
            offline_uuid("nobody_here_x"),
            "d5d44aa0-dc27-3ba6-b0e3-b2d9ccedc088"
        );
        assert!(!is_account_uuid(&offline_uuid("Steve")));
        assert!(is_account_uuid("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
    }

    #[test]
    fn uuids_in_either_form() {
        let notch = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        assert_eq!(
            normalize_uuid("069a79f444e94726a5befca90e38aaf5").as_deref(),
            Some(notch)
        );
        assert_eq!(normalize_uuid(&notch.to_uppercase()).as_deref(), Some(notch));
        for bad in [
            "",
            "069a79f4",
            "069a79f4-44e9-4726-a5be-fca90e38aaf",
            "zz9a79f444e94726a5befca90e38aaf5",
            "069a-79f444e94726a5befca90e38aaf5",
            "../../etc/passwd",
        ] {
            assert_eq!(normalize_uuid(bad), None, "{bad}");
        }
    }
}
