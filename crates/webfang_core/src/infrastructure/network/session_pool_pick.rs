//! Balanced slot-pick mechanism of the domain session pool (#1134).
//!
//! Split seam: the deterministic start index for the hash-rotated slot scan.
//! Pure std hashing with zero pool-field access; the scan itself
//! (`find_available`) stays in the parent module.

use std::time::{Duration, Instant};

/// Process-lifetime anchor for [`pick_start`]'s time signal. `Instant`
/// exposes no raw bits, so the only clock reading available is its distance
/// from another `Instant`. This is a read-only-after-init constant, not
/// per-domain state: any fixed anchor yields the same pick distribution.
static PICK_ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Deterministic start index for the balanced slot scan (#1134).
///
/// Folds the domain bytes and the clock reading (distance from the process
/// anchor, in nanoseconds) through an FNV-1a mix plus a SplitMix64 avalanche
/// finalizer, so the low bits used by the modulo carry entropy from both
/// inputs and successive acquire instants land on different slots. No state
/// is kept between calls: the same `(domain, now)` pair always resolves to
/// the same start, keeping picks reproducible under an injected clock.
pub(super) fn pick_start(domain: &str, now: Instant, len: usize) -> usize {
    let mut seed: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
    for byte in domain.bytes() {
        seed ^= u64::from(byte);
        seed = seed.wrapping_mul(0x0000_0100_0000_01b3); // FNV-1a prime
    }
    // |now - anchor| in nanos. The reverse direction covers injected clocks
    // frozen before the anchor was captured (tests seed `Instant::now()` a
    // few microseconds earlier than the first acquire).
    let anchor = *PICK_ANCHOR.get_or_init(Instant::now);
    let tick = now
        .checked_duration_since(anchor)
        .or_else(|| anchor.checked_duration_since(now))
        .unwrap_or(Duration::ZERO);
    seed ^= u64::try_from(tick.as_nanos()).unwrap_or(u64::MAX);
    // SplitMix64 finalizer — bijective avalanche over the mixed seed.
    let mut z = seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // `len` is ≥ 1 by construction (DomainSlots is NonZero); the guards keep
    // the narrowing casts infallible without introducing an unwrap.
    let modulus = u64::try_from(len.max(1)).unwrap_or(1);
    usize::try_from(z % modulus).unwrap_or(0)
}

#[cfg(test)]
#[path = "session_pool_pick_tests.rs"]
mod tests;
