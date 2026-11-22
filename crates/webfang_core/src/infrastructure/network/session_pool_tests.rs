use super::*;
use crate::domain::budget::MAX_TRACKED_DOMAINS;
use crate::domain::clock::MockClock;

/// Test constructor for the NonZero-gated Domain tier.
pub(super) fn slots(n: usize) -> DomainSlots {
    DomainSlots::new(n).expect("test slot count non-zero")
}

// ── Task 3.3: State transitions ──

#[test]
fn new_session_is_healthy() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").expect("should acquire");
    // #1134: the pick is hash-rotated, so any seeded slot is a valid
    // answer — the invariant is "acquire returns one of the healthy
    // slots", not "always slot 0".
    assert!(id.0 < 8, "pick must stay inside the seeded slots");
}

#[test]
fn healthy_to_banned_on_429() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    let sessions = pool.sessions.get("example.com").unwrap();
    assert_eq!(sessions.states[id.0].status, SessionStatus::Banned);
    assert_eq!(sessions.states[id.0].consecutive_failures, 1);
}

#[test]
fn banned_to_healthy_on_success() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);
    pool.report_success("example.com", id);

    let sessions = pool.sessions.get("example.com").unwrap();
    assert_eq!(sessions.states[id.0].status, SessionStatus::Healthy);
    assert_eq!(sessions.states[id.0].consecutive_failures, 0);
}

#[test]
fn non_ban_failure_does_not_ban() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 500);

    let sessions = pool.sessions.get("example.com").unwrap();
    assert_eq!(sessions.states[id.0].status, SessionStatus::Healthy);
    assert_eq!(sessions.states[id.0].consecutive_failures, 1);
}

#[test]
fn repeated_non_ban_failures_trigger_retiring() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 500);
    pool.report_failure("example.com", id, 500);
    pool.report_failure("example.com", id, 500);

    let sessions = pool.sessions.get("example.com").unwrap();
    assert_eq!(sessions.states[id.0].status, SessionStatus::Retiring);
}

// ── Task 3.3: Backoff doubling (with ±20% jitter) ──

#[test]
fn backoff_doubles_with_failures() {
    let pool = DomainSessionPool::default_pool();
    // With ±20% jitter, d1 should be in [1.6s, 2.4s], d2 in [3.2s, 4.8s], d3 in [6.4s, 9.6s]
    let d1 = pool.backoff_delay(1);
    assert!(
        d1 >= Duration::from_millis(1600) && d1 <= Duration::from_millis(2400),
        "d1 should be ~2s ±20%, got {d1:?}"
    );
    let d2 = pool.backoff_delay(2);
    assert!(
        d2 >= Duration::from_millis(3200) && d2 <= Duration::from_millis(4800),
        "d2 should be ~4s ±20%, got {d2:?}"
    );
    let d3 = pool.backoff_delay(3);
    assert!(
        d3 >= Duration::from_millis(6400) && d3 <= Duration::from_millis(9600),
        "d3 should be ~8s ±20%, got {d3:?}"
    );
}

#[test]
fn backoff_capped_at_max_delay() {
    let config = SessionPoolConfig {
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(10),
        max_exp: 6,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    // With ±20% jitter, capped delay should be in [8s, 12s]
    let d_large = pool.backoff_delay(100);
    assert!(
        d_large >= Duration::from_secs(8) && d_large <= Duration::from_secs(12),
        "capped delay should be ~10s ±20%, got {d_large:?}"
    );
}

#[test]
fn backoff_uses_max_exp_cap() {
    let config = SessionPoolConfig {
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(120),
        max_exp: 4,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    // max_exp=4 means exponent is capped at 4, so 2^4=16
    // With ±20% jitter, delay should be in [12.8s, 19.2s]
    let d4 = pool.backoff_delay(4);
    assert!(
        d4 >= Duration::from_millis(12800) && d4 <= Duration::from_millis(19200),
        "d4 should be ~16s ±20%, got {d4:?}"
    );
    let d5 = pool.backoff_delay(5);
    assert!(
        d5 >= Duration::from_millis(12800) && d5 <= Duration::from_millis(19200),
        "d5 should be ~16s ±20%, got {d5:?}"
    );
    let d6 = pool.backoff_delay(6);
    assert!(
        d6 >= Duration::from_millis(12800) && d6 <= Duration::from_millis(19200),
        "d6 should be ~16s ±20%, got {d6:?}"
    );
}

// ── Task 3.3: TTL eviction ──

#[test]
fn stale_sessions_evicted_on_acquire() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            ttl_duration: Duration::from_millis(10),
            ..Default::default()
        },
        clock.handle(),
    );
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // Advance past TTL — the shared clock updates the pool's view too
    clock.advance(Duration::from_millis(20));

    // acquire should evict the stale banned session and return a healthy one
    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "stale session should be evicted and retried");
}

/// #1130 contract: `evict_stale` no longer only resets session states —
/// a domain idle past the TTL loses its entry outright.
#[test]
fn evict_stale_removes_old_banned_sessions() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            ttl_duration: Duration::from_millis(10),
            ..Default::default()
        },
        clock.handle(),
    );
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // Advance past TTL
    clock.advance(Duration::from_millis(20));
    pool.evict_stale();

    assert!(
        pool.sessions.get("example.com").is_none(),
        "domain idle past TTL must be removed, not just reset"
    );
    assert_eq!(pool.total_domains(), 0);
}

#[test]
fn healthy_sessions_not_evicted_by_ttl() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            ttl_duration: Duration::from_millis(10),
            ..Default::default()
        },
        clock.handle(),
    );
    let _id = pool.acquire("example.com").unwrap();

    // Stay inside the TTL — the domain is fresh and must survive
    clock.advance(Duration::from_millis(5));
    pool.evict_stale();

    let sessions = pool.sessions.get("example.com").unwrap();
    assert_eq!(sessions.states[0].status, SessionStatus::Healthy);
}

// ── #1130: bounded domain map (cap + real eviction) ──

/// Soak: a long-lived server sees unique domains forever. Before the fix
/// the map grew linearly (one `Vec` per domain, never removed); now the
/// shared `MAX_TRACKED_DOMAINS` cap holds the size flat at the bound.
#[cfg_attr(
    miri,
    ignore = "1000 interpreted inserts with ~500 scan+sort eviction passes hang Miri past the lane cap (run 35896922208); size-plateau shape, meaningless under an interpreter"
)]
#[test]
fn soak_unique_domains_stays_bounded() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(1),
            ..Default::default()
        },
        clock.handle(),
    );
    for i in 0..(2 * MAX_TRACKED_DOMAINS) {
        clock.advance(Duration::from_micros(1));
        let domain = format!("domain-{i}.example.com");
        let _ = pool.acquire(&domain);
    }
    assert_eq!(
        pool.total_domains(),
        MAX_TRACKED_DOMAINS,
        "tracked domains must plateau at the cap, not grow linearly"
    );
}

/// The cap evicts the LEAST-recently-seen domain: the oldest untouched
/// entry is gone, the newest entries (including the insert that crossed
/// the cap) survive.
#[test]
fn cap_evicts_least_recently_seen_domain() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(1),
            ..Default::default()
        },
        clock.handle(),
    );
    for i in 0..MAX_TRACKED_DOMAINS {
        clock.advance(Duration::from_micros(1));
        let domain = format!("domain-{i}.example.com");
        let _ = pool.acquire(&domain);
    }
    assert_eq!(pool.total_domains(), MAX_TRACKED_DOMAINS);

    // One more unique domain crosses the cap → oldest is removed.
    clock.advance(Duration::from_micros(1));
    let _ = pool.acquire("domain-new.example.com");
    assert_eq!(pool.total_domains(), MAX_TRACKED_DOMAINS);
    assert!(
        pool.sessions.get("domain-0.example.com").is_none(),
        "the least-recently-seen domain must be the victim"
    );
    assert!(pool.sessions.get("domain-new.example.com").is_some());
    assert!(
        pool.sessions
            .get(&format!("domain-{}.example.com", MAX_TRACKED_DOMAINS - 1))
            .is_some(),
        "the freshest pre-cap domain must survive"
    );
}

/// Activity touches (`report_success` / `report_failure`) refresh
/// `last_seen`, keeping a busy domain ahead of cap eviction while an
/// idle one becomes the victim.
#[test]
fn touched_domain_survives_cap_eviction() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(1),
            ..Default::default()
        },
        clock.handle(),
    );
    let _ = pool.acquire("busy.example.com").unwrap();
    for i in 1..MAX_TRACKED_DOMAINS {
        clock.advance(Duration::from_micros(1));
        let domain = format!("domain-{i}.example.com");
        let _ = pool.acquire(&domain);
    }
    // busy.example.com is the OLDEST entry — touch it to keep it alive.
    clock.advance(Duration::from_micros(1));
    pool.report_success("busy.example.com", SessionId(0));
    clock.advance(Duration::from_micros(1));
    let _ = pool.acquire("domain-overflow.example.com");

    assert_eq!(pool.total_domains(), MAX_TRACKED_DOMAINS);
    assert!(
        pool.sessions.get("busy.example.com").is_some(),
        "a touched domain must outrank an idle one for eviction"
    );
    assert!(
        pool.sessions.get("domain-1.example.com").is_none(),
        "the now-oldest idle domain must be the victim"
    );
}

/// `evict_stale` removes idle domains outright and keeps fresh ones —
/// the map size drops, it is not merely a state reset.
#[test]
fn evict_stale_removes_idle_domains_keeps_fresh() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            ttl_duration: Duration::from_millis(10),
            ..Default::default()
        },
        clock.handle(),
    );
    let _ = pool.acquire("idle.example.com").unwrap();
    clock.advance(Duration::from_millis(15));
    let _ = pool.acquire("fresh.example.com").unwrap();

    pool.evict_stale();

    assert!(pool.sessions.get("idle.example.com").is_none());
    assert!(pool.sessions.get("fresh.example.com").is_some());
    assert_eq!(pool.total_domains(), 1);
}
