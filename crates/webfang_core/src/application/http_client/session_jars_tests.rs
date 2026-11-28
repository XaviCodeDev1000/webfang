//! Bounds of the session-jar store: cap, release, size and expiry.
//!
//! The store is a data structure, so its tests read its own interface
//! (`live_jars`, `cookie_count`, `jar_for`); the scope's end-to-end behavior is
//! pinned on the wire by `tests/cookie_jar_session_test.rs`.

#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::domain::clock::MockClock;

/// A URL for the fixture host; the store only ever filters cookies by it.
const HOST: &str = "https://example.com/";

/// Limits with only the field under test changed.
fn limits(max_jars: usize) -> SessionJarLimits {
    SessionJarLimits {
        max_jars,
        ..SessionJarLimits::default()
    }
}

fn add(jar: &Jar, cookies: &[(&str, &str)]) {
    for (name, value) in cookies {
        jar.add(format!("{name}={value}"), HOST);
    }
}

/// The cookie names a request to [`HOST`] would carry, sorted.
fn offered(jar: &Jar) -> Vec<String> {
    let mut names: Vec<String> = jar
        .matches(HOST)
        .map(|cookie| cookie.name().to_string())
        .collect();
    names.sort();
    names
}

/// A store on a mock clock, with a 30s idle window.
fn store_on_clock(clock: &MockClock) -> SessionJarStore {
    SessionJarStore::with_clock(
        SessionJarLimits {
            idle_ttl: Duration::from_secs(30),
            ..SessionJarLimits::default()
        },
        clock.handle(),
    )
}

#[test]
fn the_defaults_mirror_the_transports_own_session_bounds() {
    let limits = SessionJarLimits::default();
    assert_eq!(limits.max_jars, DEFAULT_MAX_SESSION_JARS);
    assert_eq!(limits.idle_ttl.as_secs(), DEFAULT_SESSION_JAR_IDLE_TTL_SECS);
    // rmcp's SessionConfig::DEFAULT_KEEP_ALIVE: a session idle that long is
    // already reaped, so its jar must not outlive it.
    assert_eq!(limits.idle_ttl.as_secs(), 300);
    assert!(limits.max_cookies_per_jar > 0);
    assert!(limits.max_cookie_bytes_per_jar > 0);
}

#[test]
fn each_session_gets_its_own_jar() {
    let store = SessionJarStore::with_defaults();

    let first = store.jar_for("session-a", None, HOST);
    add(&first, &[("a", "1")]);
    let second = store.jar_for("session-b", None, HOST);

    assert_eq!(store.cookie_count("session-a"), Some(1));
    assert_eq!(store.cookie_count("session-b"), Some(0));
    assert!(second.is_empty(), "a new session starts with no cookies");
    assert_eq!(store.live_jars(), 2);
}

#[test]
fn a_repeated_lookup_returns_the_same_jar() {
    let store = SessionJarStore::with_defaults();
    let jar = store.jar_for("session-a", None, HOST);
    add(&jar, &[("a", "1")]);

    let again = store.jar_for("session-a", None, HOST);

    assert_eq!(
        store.cookie_count("session-a"),
        Some(1),
        "the second lookup must not replace the jar"
    );
    assert_eq!(offered(&again), vec!["a".to_owned()]);
}

#[test]
fn the_seed_is_applied_once_at_creation_and_only_for_its_host() {
    let store = SessionJarStore::with_defaults();
    let seed = Jar::default();
    seed.add("seed=1", HOST);

    let jar = store.jar_for("session-a", Some(&seed), HOST);
    jar.add("site=2", HOST);
    // A later request on the same session must not re-apply the seed over what
    // the site has since set.
    let again = store.jar_for("session-a", Some(&seed), HOST);

    assert_eq!(offered(&again), vec!["seed".to_owned(), "site".to_owned()]);
    assert_eq!(
        store
            .jar_for("session-b", Some(&seed), "https://other.example.org/")
            .len(),
        0,
        "a seed for one host never leaks to another"
    );
}

#[test]
fn dropping_the_lease_releases_the_sessions_jar() {
    let store = Arc::new(SessionJarStore::with_defaults());
    let lease = store.lease("session-a");
    let jar = store.jar_for(lease.session(), None, HOST);
    add(&jar, &[("a", "1")]);
    assert_eq!(store.live_jars(), 1);

    drop(lease);

    assert_eq!(
        store.live_jars(),
        0,
        "the session's jar is gone with its lease"
    );
    assert_eq!(store.cookie_count("session-a"), None);
}

#[test]
fn releasing_an_unknown_session_reports_nothing_was_live() {
    let store = SessionJarStore::with_defaults();
    assert!(!store.release("never-seen"));
    store.jar_for("session-a", None, HOST);
    assert!(store.release("session-a"));
    assert!(!store.release("session-a"));
}

#[test]
fn the_cap_evicts_the_least_recently_used_jar() {
    let store = SessionJarStore::with_limits(limits(2));

    store.jar_for("session-a", None, HOST);
    store.jar_for("session-b", None, HOST);
    // Touch `session-a` WITHOUT advancing real time: the store's ordering is
    // decided by the last use it records, so a second access is the only
    // evidence available and the clock is monotonic in production.
    store.jar_for("session-a", None, HOST);
    store.jar_for("session-c", None, HOST);

    assert_eq!(store.live_jars(), 2, "the cap holds");
    assert!(
        store.cookie_count("session-b").is_none(),
        "the least recently used jar is the one evicted"
    );
    assert!(store.cookie_count("session-a").is_some());
    assert!(store.cookie_count("session-c").is_some());
}

#[test]
fn the_idle_window_expires_an_untouched_jar_and_keeps_a_used_one() {
    let clock = MockClock::new(Instant::now());
    let store = store_on_clock(&clock);
    let jar = store.jar_for("session-a", None, HOST);
    add(&jar, &[("a", "1")]);

    // A request every 20s keeps the jar inside the 30s window.
    for _ in 0..4 {
        clock.advance(Duration::from_secs(20));
        store.jar_for("session-a", None, HOST);
    }
    assert_eq!(store.live_jars(), 1);
    assert_eq!(store.cookie_count("session-a"), Some(1));

    // Then the caller goes quiet for longer than the window.
    clock.advance(Duration::from_secs(31));
    assert_eq!(store.live_jars(), 0, "an idle session's jar expires");
    let fresh = store.jar_for("session-a", None, HOST);
    assert!(
        fresh.is_empty(),
        "the session that comes back starts with no cookies"
    );
}

#[test]
fn the_size_cap_keeps_the_newest_cookies() {
    let store = SessionJarStore::with_limits(SessionJarLimits {
        max_cookies_per_jar: 2,
        ..SessionJarLimits::default()
    });
    let jar = store.jar_for("session-a", None, HOST);
    add(&jar, &[("a", "1"), ("b", "2"), ("c", "3")]);

    store.trim("session-a");

    assert_eq!(store.cookie_count("session-a"), Some(2));
    assert_eq!(
        offered(&jar),
        vec!["b".to_owned(), "c".to_owned()],
        "the cookie the site set last survives the trim"
    );
}

#[test]
fn the_byte_cap_drops_the_oldest_cookies_until_it_holds() {
    let store = SessionJarStore::with_limits(SessionJarLimits {
        max_cookie_bytes_per_jar: 12,
        ..SessionJarLimits::default()
    });
    let jar = store.jar_for("session-a", None, HOST);
    add(&jar, &[("wide", "0123456789"), ("ok", "1")]);

    store.trim("session-a");

    assert_eq!(
        offered(&jar),
        vec!["ok".to_owned()],
        "the oversized oldest cookie is dropped, the small newest one stays"
    );
}

#[test]
fn a_single_cookie_over_the_byte_cap_empties_the_jar_instead_of_keeping_it() {
    let store = SessionJarStore::with_limits(SessionJarLimits {
        max_cookie_bytes_per_jar: 4,
        ..SessionJarLimits::default()
    });
    let jar = store.jar_for("session-a", None, HOST);
    add(&jar, &[("wide", "0123456789")]);

    store.trim("session-a");

    assert_eq!(
        offered(&jar),
        Vec::<String>::new(),
        "no cookie may survive above the cap: the bound is a hard one"
    );
}

#[test]
fn trimming_an_unknown_session_is_a_no_op() {
    let store = SessionJarStore::with_defaults();
    store.trim("never-seen");
    assert_eq!(store.live_jars(), 0);
}

#[test]
fn the_store_exposes_the_limits_it_enforces() {
    let store = SessionJarStore::with_limits(limits(7));
    assert_eq!(store.limits().max_jars, 7);
}
