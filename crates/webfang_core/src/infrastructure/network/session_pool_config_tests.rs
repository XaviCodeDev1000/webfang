use super::tests::slots;
use super::*;
use crate::domain::clock::MockClock;

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
