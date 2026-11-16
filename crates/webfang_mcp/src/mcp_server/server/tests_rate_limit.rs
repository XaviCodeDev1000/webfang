//! Tests extraídos del hub `server.rs` por tema (PR 1 de #2584, movimiento puro).
//! Los cuerpos son verbatim del módulo de tests inline original.

use super::{
    build_rate_limiter, rate_key, retry_after_secs, AuthState, KeyedRateLimiter, ServerOptions,
    ANONYMOUS_RATE_KEY,
};
use std::sync::Arc;
use std::time::Duration;
#[test]
fn test_rate_limiter_allows_within_quota() {
    let opts = ServerOptions {
        rate_per_second: 10,
        rate_burst: 5,
        ..Default::default()
    };
    let limiter = build_rate_limiter(
        &opts,
        &AuthState {
            expected_token: None,
            allow_anonymous: false,
        },
    );
    let key = ANONYMOUS_RATE_KEY;
    for _ in 0..5 {
        assert!(limiter.check(&key).is_ok());
    }
}

#[test]
fn test_rate_limiter_rejects_over_burst() {
    let opts = ServerOptions {
        rate_per_second: 1,
        rate_burst: 2,
        ..Default::default()
    };
    let limiter = build_rate_limiter(
        &opts,
        &AuthState {
            expected_token: None,
            allow_anonymous: false,
        },
    );
    let key = ANONYMOUS_RATE_KEY;
    // Exhaust the burst capacity
    assert!(limiter.check(&key).is_ok());
    assert!(limiter.check(&key).is_ok());
    // Third request should be rejected
    assert!(limiter.check(&key).is_err());
}

// ------------------------------------------------------------------
// F5 keyed rate limiter, inside auth (#1611)
// ------------------------------------------------------------------

fn limiter_with_auth(rate_per_second: u32, burst: u32, token: Option<&str>) -> KeyedRateLimiter {
    build_rate_limiter(
        &ServerOptions {
            rate_per_second,
            rate_burst: burst,
            ..Default::default()
        },
        &AuthState {
            expected_token: token.map(Arc::from),
            // Irrelevant to a rate limiter: with no token configured
            // every request shares the anonymous bucket anyway.
            allow_anonymous: false,
        },
    )
}

/// The keyed property itself: one caller's exhausted quota must NOT shed
/// another caller. This is the whole of F5 — a `NotKeyed` limiter had
/// exactly one bucket, so a noisy client spent everyone's budget.
#[test]
fn rate_limit_buckets_are_per_credential() {
    let limiter = limiter_with_auth(1, 1, Some("secret"));
    let noisy = rate_key(true, Some("Bearer secret"));
    let quiet = rate_key(true, Some("Bearer another-secret"));

    assert!(limiter.check(&noisy).is_ok(), "first call fits the burst");
    assert!(
        limiter.check(&noisy).is_err(),
        "the noisy caller exhausts its own bucket"
    );
    assert!(
        limiter.check(&quiet).is_ok(),
        "a different credential must keep its own full budget"
    );
    assert_eq!(
        limiter.tracked_keys(),
        2,
        "one bucket per credential, and no more"
    );
}

/// The bound on the key space, asserted rather than assumed. With auth
/// DISABLED the presented header is attacker-controlled, so keying on it
/// would let a client mint a fresh full quota per invented header — an
/// unbounded map of buckets, i.e. the F6 shape one layer over. Every
/// header therefore lands in the single anonymous bucket.
#[test]
fn an_invented_header_cannot_mint_a_bucket_when_auth_is_disabled() {
    let limiter = limiter_with_auth(1, 1, None);
    for i in 0..50 {
        let key = rate_key(false, Some(&format!("Bearer invented-{i}")));
        assert_eq!(key, ANONYMOUS_RATE_KEY, "the header is ignored");
        let _ = limiter.check(&key);
    }
    assert_eq!(
        limiter.tracked_keys(),
        1,
        "token-less traffic must occupy exactly one bucket"
    );
}

/// The mirror image: with auth configured, distinct credentials are
/// distinct buckets — and the fingerprint is not the token itself, so a
/// key cannot be read back as a secret.
#[test]
fn with_auth_configured_the_credential_picks_the_bucket() {
    let one = rate_key(true, Some("Bearer secret"));
    let two = rate_key(true, Some("Bearer secret"));
    let other = rate_key(true, Some("Bearer other"));
    assert_eq!(one, two, "the same credential is the same bucket");
    assert_ne!(one, other, "different credentials never share a bucket");
    assert!(
        !ANONYMOUS_RATE_KEY.to_string().contains("secret"),
        "a key is a fingerprint, never the token"
    );
}

/// `Retry-After` must be a whole number of seconds, at least 1 — rounding
/// a sub-second wait DOWN invites the client back into a second 429.
#[test]
fn retry_after_rounds_up_and_never_says_zero() {
    assert_eq!(retry_after_secs(Duration::ZERO), 1);
    assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
    assert_eq!(retry_after_secs(Duration::from_millis(999)), 1);
    assert_eq!(retry_after_secs(Duration::from_secs(1)), 1);
    assert_eq!(retry_after_secs(Duration::from_millis(1_001)), 2);
    assert_eq!(retry_after_secs(Duration::from_secs(30)), 30);
}
