//! Slowing down password guessing at the sign-in endpoint.
//!
//! Failed sign-ins are counted per address and username, and per address across usernames. After a
//! few misses the next attempt has to wait, and the wait doubles with every further miss, up to
//! [`MAX_LOCKOUT`]. A sign-in that works clears the count for that username from that address.
//!
//! - **Per address and username:** [`FREE_ATTEMPTS`] misses are free. Counting by username alone
//!   would let anyone lock the owner out by guessing against their name.
//! - **Per address:** [`ADDRESS_ATTEMPTS`] misses within [`WINDOW`], whatever the usernames, stops
//!   one address from trying one password against many names.
//!
//! An IPv6 address counts as its /64 network: one machine has that many addresses to rotate through.
//! Behind a reverse proxy the address is the proxy's unless `trusted_proxies` names it (see
//! `client.rs`), and then every visitor shares these counts. Everything is in memory: a restart
//! forgets it, which costs an attacker far more time than it gains.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Wrong passwords for one username from one address before waiting starts.
pub const FREE_ATTEMPTS: u32 = 5;
/// Wrong passwords from one address, across usernames, before it has to wait.
pub const ADDRESS_ATTEMPTS: u32 = 30;
/// Misses older than this no longer count.
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// The wait after the first miss beyond the free ones; it doubles from there.
const FIRST_LOCKOUT: Duration = Duration::from_secs(30);
const MAX_LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Counts kept at most. Past this, the expired ones go; if that isn't enough, all of them.
const MAX_ENTRIES: usize = 50_000;

/// Whose misses a count is of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Address(Option<IpAddr>),
    AddressAndUsername(Option<IpAddr>, String),
}

#[derive(Debug, Clone, Copy)]
struct Misses {
    count: u32,
    last: Instant,
    blocked_until: Option<Instant>,
}

/// The counts of failed sign-ins. One per app, shared by every request.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    misses: Mutex<HashMap<Key, Misses>>,
}

impl LoginThrottle {
    /// Whether a sign-in may be tried now; if not, how long until it may.
    pub fn check(&self, address: Option<IpAddr>, username: &str, now: Instant) -> Result<(), Duration> {
        let misses = self.lock();
        keys(address, username)
            .iter()
            .filter_map(|key| misses.get(key)?.blocked_until)
            .filter(|until| *until > now)
            .max()
            .map_or(Ok(()), |until| Err(until - now))
    }

    /// Counts a wrong password.
    pub fn record_failure(&self, address: Option<IpAddr>, username: &str, now: Instant) {
        let mut misses = self.lock();
        if misses.len() >= MAX_ENTRIES {
            misses.retain(|_, entry| !entry.is_over(now));
            if misses.len() >= MAX_ENTRIES {
                tracing::warn!("too many failed sign-ins to keep count of; starting over");
                misses.clear();
            }
        }
        let [by_address, by_pair] = keys(address, username);
        for (key, free) in [(by_address, ADDRESS_ATTEMPTS), (by_pair, FREE_ATTEMPTS)] {
            let entry = misses.entry(key).or_insert(Misses {
                count: 0,
                last: now,
                blocked_until: None,
            });
            if entry.is_over(now) {
                entry.count = 0;
            }
            entry.count += 1;
            entry.last = now;
            entry.blocked_until = lockout(entry.count, free).map(|wait| now + wait);
        }
    }

    /// A sign-in worked: that username's misses from that address are forgiven. The address's
    /// own count stays, or one valid account would reset guessing against the others.
    pub fn record_success(&self, address: Option<IpAddr>, username: &str) {
        let [_, by_pair] = keys(address, username);
        self.lock().remove(&by_pair);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Misses>> {
        self.misses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Misses {
    /// Too old to count, and not holding anyone back any more.
    fn is_over(&self, now: Instant) -> bool {
        let blocked = self.blocked_until.is_some_and(|until| until > now);
        !blocked && now.duration_since(self.last) > WINDOW
    }
}

/// How long to wait after `count` misses when `free` of them cost nothing.
fn lockout(count: u32, free: u32) -> Option<Duration> {
    let beyond = count.checked_sub(free).filter(|beyond| *beyond > 0)?;
    let doubled = FIRST_LOCKOUT.saturating_mul(2_u32.saturating_pow(beyond - 1));
    Some(doubled.min(MAX_LOCKOUT))
}

fn keys(address: Option<IpAddr>, username: &str) -> [Key; 2] {
    let address = address.map(network);
    // Usernames are case-insensitive when signing in, so they are when counting.
    let username = username.trim().to_lowercase();
    [Key::Address(address), Key::AddressAndUsername(address, username)]
}

/// The address itself for IPv4; the /64 it is in for IPv6.
fn network(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(segments.into())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    // The throttle takes an `Option`: a request may come with no known address.
    #[allow(clippy::unnecessary_wraps)]
    fn address(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    fn miss(throttle: &LoginThrottle, from: Option<IpAddr>, username: &str, times: u32, now: Instant) {
        for _ in 0..times {
            throttle.record_failure(from, username, now);
        }
    }

    #[test]
    fn a_few_misses_are_free_then_the_wait_doubles_up_to_a_limit() {
        let throttle = LoginThrottle::default();
        let (from, now) = (address("203.0.113.9"), Instant::now());

        miss(&throttle, from, "owner", FREE_ATTEMPTS, now);
        assert_eq!(throttle.check(from, "owner", now), Ok(()));

        throttle.record_failure(from, "owner", now);
        assert_eq!(throttle.check(from, "owner", now), Err(FIRST_LOCKOUT));
        assert_eq!(
            throttle.check(from, "owner", now + FIRST_LOCKOUT),
            Ok(()),
            "the wait ends"
        );

        throttle.record_failure(from, "owner", now);
        assert_eq!(throttle.check(from, "owner", now), Err(FIRST_LOCKOUT * 2));
        miss(&throttle, from, "owner", 20, now);
        assert_eq!(throttle.check(from, "owner", now), Err(MAX_LOCKOUT));
    }

    #[test]
    fn a_wait_is_for_that_username_from_that_address_only() {
        let throttle = LoginThrottle::default();
        let (attacker, now) = (address("203.0.113.9"), Instant::now());
        miss(&throttle, attacker, "Owner", FREE_ATTEMPTS + 1, now);

        assert!(
            throttle.check(attacker, " owner ", now).is_err(),
            "the same name, spelled another way"
        );
        assert_eq!(
            throttle.check(address("198.51.100.7"), "owner", now),
            Ok(()),
            "the owner signs in from home: guessing against their name doesn't lock them out"
        );
        assert_eq!(
            throttle.check(attacker, "friend", now),
            Ok(()),
            "another account from the same address"
        );
    }

    #[test]
    fn one_address_can_not_go_through_many_usernames() {
        let throttle = LoginThrottle::default();
        let (from, now) = (address("203.0.113.9"), Instant::now());
        for user in 0..ADDRESS_ATTEMPTS {
            assert_eq!(throttle.check(from, &format!("user{user}"), now), Ok(()));
            throttle.record_failure(from, &format!("user{user}"), now);
        }
        throttle.record_failure(from, "one-more", now);
        assert!(throttle.check(from, "never-tried", now).is_err());
        assert_eq!(
            throttle.check(address("203.0.113.10"), "never-tried", now),
            Ok(())
        );
    }

    #[test]
    fn signing_in_forgives_that_username_and_old_misses_stop_counting() {
        let throttle = LoginThrottle::default();
        let (from, now) = (address("203.0.113.9"), Instant::now());

        miss(&throttle, from, "owner", FREE_ATTEMPTS, now);
        throttle.record_success(from, "OWNER");
        miss(&throttle, from, "owner", FREE_ATTEMPTS, now);
        assert_eq!(
            throttle.check(from, "owner", now),
            Ok(()),
            "counted from nothing again"
        );

        let later = now + WINDOW + Duration::from_secs(1);
        throttle.record_failure(from, "owner", later);
        assert_eq!(
            throttle.check(from, "owner", later),
            Ok(()),
            "the misses of a quarter of an hour ago no longer count"
        );
    }

    #[test]
    fn an_ipv6_network_counts_as_one_address() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        for host in 0..=FREE_ATTEMPTS {
            let from = address(&format!("2001:db8:1:2::{host:x}"));
            throttle.record_failure(from, "owner", now);
        }
        assert!(
            throttle
                .check(address("2001:db8:1:2:ffff::1"), "owner", now)
                .is_err(),
            "another address of the same /64"
        );
        assert_eq!(throttle.check(address("2001:db8:1:3::1"), "owner", now), Ok(()));
    }

    #[test]
    fn the_counts_stay_bounded() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        for host in 0..u32::try_from(MAX_ENTRIES).unwrap() {
            let from = Some(IpAddr::V4(host.into()));
            throttle.record_failure(from, "owner", now);
        }
        assert!(throttle.lock().len() <= MAX_ENTRIES + 2);
    }
}
