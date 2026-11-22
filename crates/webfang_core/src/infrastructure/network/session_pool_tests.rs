use super::*;
use crate::domain::budget::MAX_TRACKED_DOMAINS;
use crate::domain::clock::MockClock;

/// Test constructor for the NonZero-gated Domain tier.
fn slots(n: usize) -> DomainSlots {
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

// ── ADR-0012-B 3.F: from_domain_config maps the domain DTO ──

/// The composition-root seam builds the pool from the domain config DTO.
/// `pool_size` is observed through the seeded slot count (the default
/// would seed 8); `base_delay` through the ban cooldown — with a single
/// slot the ban covers the whole domain, and the 1s default would have
/// recovered well before the 120s advance below. `max_delay` is raised
/// alongside so the 60s default cap does not mask the delay assertion.
#[test]
fn from_domain_config_maps_pool_size_and_delays() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::from_domain_config(
        crate::domain::session_port::SessionPoolConfig {
            pool_size: slots(1),
            base_delay: Duration::from_secs(3600),
            max_delay: Duration::from_secs(7200),
            ..Default::default()
        },
        clock.handle(),
    );

    let id = pool.acquire("example.com").expect("healthy session");
    assert_eq!(
        pool.domain_count("example.com"),
        1,
        "domain DTO pool_size must seed exactly that many slots"
    );

    pool.report_failure("example.com", id, 429);
    clock.advance(Duration::from_secs(120));
    assert!(
        pool.acquire("example.com").is_none(),
        "mapped 1h base_delay must keep the session banned past 120s \
         (the 1s default would already have recovered)"
    );
}

// ── Task 2.2(c): pool size follows the budget model's Domain tier ──

/// Task 2.2(c): the pool's slot count follows the injected [`BudgetModel`]
/// Domain tier — a raw `usize` is no longer representable (D4 newtype),
/// so a miswired or zero slot count cannot compile.
#[test]
fn pool_size_follows_budget_model_domain_tier() {
    use std::num::NonZeroUsize;

    use crate::domain::budget::detector::FixedDetector;
    use crate::domain::budget::{BudgetModel, BudgetOverrides};

    fn config_from_cores(cores: usize) -> (SessionPoolConfig, usize) {
        let detector = FixedDetector::with_detection(
            NonZeroUsize::new(cores).expect("test cores non-zero"),
            None,
        );
        let model = BudgetModel::build(BudgetOverrides::default(), &detector);
        let expected = model.domain().get();
        let config = SessionPoolConfig {
            base_delay: Duration::from_secs(1),
            pool_size: model.domain(),
            ..SessionPoolConfig::default()
        };
        (config, expected)
    }

    // TRIANGULATE across detector variants — every variant must resolve
    // the pool to exactly its model's Domain tier.
    for cores in [2, 4, 16] {
        let (config, expected) = config_from_cores(cores);
        assert_eq!(config.pool_size.get(), expected);

        let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
        let _id = pool.acquire("example.com").expect("should acquire");
        assert_eq!(
            pool.domain_count("example.com"),
            expected,
            "cores={cores}: live slot count must equal the model Domain tier"
        );
    }

    // Zero slots are unrepresentable: the NonZero guard rejects them at
    // construction instead of silently degrading the pool.
    assert!(DomainSlots::new(0).is_err());
}

// ── Task 3.3: Pool size limit ──

#[test]
fn pool_respects_configured_size() {
    let config = SessionPoolConfig {
        pool_size: slots(3),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let _id = pool.acquire("example.com").unwrap();

    assert_eq!(pool.domain_count("example.com"), 3);
}

#[test]
fn multiple_domains_independent() {
    let pool = DomainSessionPool::default_pool();
    let id1 = pool.acquire("a.com").unwrap();
    let id2 = pool.acquire("b.com").unwrap();

    pool.report_failure("a.com", id1, 429);
    pool.report_success("b.com", id2);

    // a.com has a banned session, b.com is healthy — checked at the
    // slots that were actually picked (#1134: the pick is hash-rotated).
    let a = pool.sessions.get("a.com").unwrap();
    let b = pool.sessions.get("b.com").unwrap();
    assert_eq!(a.states[id1.0].status, SessionStatus::Banned);
    assert_eq!(b.states[id2.0].status, SessionStatus::Healthy);
    assert_eq!(pool.total_domains(), 2);
}

// ── Additional edge cases ──

#[test]
fn acquire_uninitialized_domain_creates_pool() {
    let pool = DomainSessionPool::default_pool();
    assert_eq!(pool.domain_count("new.com"), 0);
    let _id = pool.acquire("new.com");
    assert_eq!(pool.domain_count("new.com"), 8);
}

#[test]
fn banned_session_available_after_cooldown() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(1),
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            max_exp: 1,
            ..Default::default()
        },
        clock.handle(),
    );
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // Verify it's banned before cooldown
    {
        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions.states[0].status, SessionStatus::Banned);
        assert!(sessions.states[0].next_retry_time.is_some());
    }

    // Advance past cooldown (backoff is 2^1 * 1ms = 2ms)
    clock.advance(Duration::from_millis(50));

    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "should be available after cooldown");
}

// ── #1134: deterministic cooldown + balanced slot pick ──

/// Symptom 1 (cooldown window mobile): with the SAME frozen state, the
/// availability verdict must not flip between calls. The base code
/// re-drew the jitter rand inside `banned_session_ready` on every check,
/// so the effective deadline moved each `acquire` — the same instant
/// answered available/not-available at random.
#[test]
fn cooldown_verdict_is_deterministic_for_frozen_state() {
    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(2),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            max_exp: 6,
            ..Default::default()
        },
        clock.handle(),
    );
    // Seed the domain entry (report_failure is a no-op for unknown
    // domains — only acquire creates the slot Vec).
    pool.acquire("example.com").expect("seed domain slots");
    // Ban slot 0 twice (deeper backoff) and slot 1 once, at the same
    // instant. Slot 0's deadline ∈ [320,480]ms, slot 1's ∈ [160,240]ms.
    pool.report_failure("example.com", SessionId(0), 429);
    pool.report_failure("example.com", SessionId(0), 429);
    pool.report_failure("example.com", SessionId(1), 429);

    let (retry0, retry1) = {
        let sessions = pool.sessions.get("example.com").unwrap();
        (
            sessions.states[0].next_retry_time.expect("slot 0 banned"),
            sessions.states[1].next_retry_time.expect("slot 1 banned"),
        )
    };
    // Freezing the clock 20ms past slot 1's base deadline lands inside
    // the jitter band, while slot 0 is provably still banned (the
    // doubling separates the two deadlines by ≥60ms).
    assert!(retry0 > retry1 + Duration::from_millis(20));
    clock.set_now(retry1 + Duration::from_millis(20));

    let verdicts: Vec<bool> = (0..40)
        .map(|_| pool.acquire("example.com").is_some())
        .collect();
    assert!(
        verdicts.iter().all(|v| *v == verdicts[0]),
        "frozen state must yield one verdict, got {verdicts:?}"
    );
}

/// Symptoms 2+3 (idx-0 stampede): the base `find_available` scanned in
/// slot order and returned the first healthy slot, so every acquire
/// collapsed onto `SessionId(0)` while the other 7 slots sat idle. The
/// hash-based pick must spread picks across slots with zero new state.
#[test]
fn acquire_spreads_picks_across_slots() {
    use std::collections::HashSet;

    let clock = MockClock::new(Instant::now());
    let pool = DomainSessionPool::new(
        SessionPoolConfig {
            pool_size: slots(8),
            ..Default::default()
        },
        clock.handle(),
    );
    let mut picks = HashSet::new();
    for _ in 0..32 {
        clock.advance(Duration::from_millis(1));
        let id = pool.acquire("example.com").expect("healthy slots");
        assert!(id.0 < 8, "pick must stay inside the seeded slots");
        picks.insert(id.0);
    }
    assert!(
        picks.len() > 1,
        "32 acquires collapsed onto the single slot {picks:?}"
    );

    // Determinism: the same frozen (domain, now) re-picks the same slot.
    let a = pool.acquire("example.com");
    let b = pool.acquire("example.com");
    assert_eq!(a, b, "pick must be reproducible for a frozen clock");
}

#[test]
fn session_id_display() {
    let id = SessionId(42);
    assert_eq!(format!("{id}"), "session-42");
}

#[test]
fn default_config_values() {
    let config = SessionPoolConfig::default();
    assert_eq!(config.pool_size.get(), 8);
    assert_eq!(config.base_delay, Duration::from_secs(1));
    assert_eq!(config.max_delay, Duration::from_secs(60));
    assert_eq!(config.max_exp, 6);
    assert_eq!(config.ttl_duration, Duration::from_secs(300));
}

#[test]
fn report_failure_on_nonexistent_session_no_panic() {
    let pool = DomainSessionPool::default_pool();
    // Should not panic — just a no-op
    pool.report_failure("ghost.com", SessionId(99), 500);
}

#[test]
fn report_success_on_nonexistent_session_no_panic() {
    let pool = DomainSessionPool::default_pool();
    pool.report_success("ghost.com", SessionId(99));
}
