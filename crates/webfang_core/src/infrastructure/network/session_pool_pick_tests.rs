//! Mutation-pinning tests for the balanced slot pick (#1134).
//!
//! [`pick_start`] is the hash-rotated start of the pool scan: its contract is
//! that a `(domain, tick)` pair maps deterministically into `0..len` and that
//! successive ticks spread the scan across every slot. The pool-level tests
//! only prove "more than one slot", so a mutation of any bit op in the
//! FNV-1a/SplitMix64 mix still passes them. These vectors pin the exact
//! mapping: a deliberate retune of the hash updates them, an accidental one
//! fails fast instead of silently degrading the distribution.

use std::time::{Duration, Instant};

use super::{pick_start, PICK_ANCHOR};

/// `now` exactly `nanos` after the process anchor, so the tick fold observes
/// `nanos` regardless of when the anchor was first initialized.
fn at_nanos(nanos: u64) -> Instant {
    let anchor = *PICK_ANCHOR.get_or_init(Instant::now);
    anchor + Duration::from_nanos(nanos)
}

#[test]
fn pick_start_matches_golden_vectors() {
    // (domain, tick in nanoseconds, slot count) -> expected start index.
    let cases = [
        ("example.com", 0_u64, 8_usize, 7_usize),
        ("example.com", 1, 8, 1),
        ("example.com", 999, 8, 1),
        ("example.com", 1_000_000, 8, 0),
        ("example.com", 86_400_000_000_000, 8, 0),
        ("other.example", 1, 8, 4),
        ("example.com", 0, 1, 0),
        ("example.com", 999, 3, 2),
        ("example.com", 1_000_000, 17, 9),
    ];
    for (domain, nanos, len, expected) in cases {
        assert_eq!(
            pick_start(domain, at_nanos(nanos), len),
            expected,
            "pick_start({domain:?}, +{nanos}ns, {len})"
        );
    }
}

#[test]
fn pick_start_covers_every_slot_on_successive_ticks() {
    // 64 consecutive nanoseconds over 8 slots: the wrap-around scan can only
    // balance the pool if every slot is reachable from the pick.
    let mut mask = 0_u8;
    for nanos in 0..64_u64 {
        let start = pick_start("example.com", at_nanos(nanos), 8);
        assert!(start < 8, "pick {start} outside 0..8");
        mask |= 1 << start;
    }
    assert_eq!(mask, 0b1111_1111, "consecutive ticks missed a slot");
}

#[test]
fn pick_start_is_reproducible_for_a_frozen_instant() {
    let now = at_nanos(123_456_789);
    assert_eq!(
        pick_start("example.com", now, 8),
        pick_start("example.com", now, 8),
        "the same (domain, now) pair must re-pick the same start"
    );
}
