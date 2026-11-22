use super::*;
use crate::env::assert_entry_guard_armed;
use std::env;

#[test]
fn env_guard_with_sets_and_restores() {
    const VAR: &str = "WEBFANG_TEST_VAR_1";
    {
        let _lock = env_lock();
        env::remove_var(VAR);
    }

    {
        let _guard = EnvGuard::with(&[(VAR, "hello")]);
        assert_eq!(env::var(VAR).unwrap(), "hello");
    }

    assert!(env::var(VAR).is_err());
}

#[test]
fn env_guard_with_restores_preexisting_value() {
    const VAR: &str = "WEBFANG_TEST_VAR_2";
    {
        let _lock = env_lock();
        env::set_var(VAR, "original");
    }

    {
        let _guard = EnvGuard::with(&[(VAR, "modified")]);
        assert_eq!(env::var(VAR).unwrap(), "modified");
    }

    assert_eq!(env::var(VAR).unwrap(), "original");
    {
        let _lock = env_lock();
        env::remove_var(VAR);
    }
}

#[test]
fn env_guard_clean_removes_and_restores() {
    const VAR: &str = "WEBFANG_TEST_VAR_3";
    {
        let _lock = env_lock();
        env::set_var(VAR, "present");
    }

    {
        let _guard = EnvGuard::clean(&[VAR]);
        assert!(env::var(VAR).is_err());
    }

    assert_eq!(env::var(VAR).unwrap(), "present");
    {
        let _lock = env_lock();
        env::remove_var(VAR);
    }
}

#[test]
fn sequential_guards_do_not_interfere() {
    const VAR_A: &str = "WEBFANG_TEST_VAR_4";
    const VAR_B: &str = "WEBFANG_TEST_VAR_5";
    {
        let _lock = env_lock();
        env::remove_var(VAR_A);
        env::remove_var(VAR_B);
    }

    {
        let _guard = EnvGuard::with(&[(VAR_A, "a")]);
        assert_eq!(env::var(VAR_A).unwrap(), "a");
    }
    assert!(env::var(VAR_A).is_err());

    {
        let _guard = EnvGuard::with(&[(VAR_B, "b")]);
        assert_eq!(env::var(VAR_B).unwrap(), "b");
        assert!(env::var(VAR_A).is_err());
    }
    assert!(env::var(VAR_B).is_err());
}

/// `set`/`remove` mutate under the lock the guard already holds, and
/// drop restores in reverse capture order so the true original wins.
#[test]
fn guard_set_and_remove_restore_the_true_original() {
    const VAR: &str = "WEBFANG_TEST_VAR_6";
    {
        let _lock = env_lock();
        env::set_var(VAR, "original");
    }

    {
        let mut guard = EnvGuard::clean(&[VAR]);
        assert!(env::var(VAR).is_err());
        guard.set(VAR, "flipped");
        assert_eq!(env::var(VAR).unwrap(), "flipped");
        guard.remove(VAR);
        assert!(env::var(VAR).is_err());
    }

    assert_eq!(env::var(VAR).unwrap(), "original");
    {
        let _lock = env_lock();
        env::remove_var(VAR);
    }
}

/// `env_set`/`env_remove` are permanent: no restore-on-drop, and each
/// call serializes itself under ENV_LOCK (no surrounding `env_lock`).
#[test]
fn env_set_and_env_remove_are_permanent() {
    const VAR: &str = "WEBFANG_TEST_VAR_7";
    {
        let _lock = env_lock();
        env::remove_var(VAR);
    }

    env_set(VAR, "permanent");
    assert_eq!(env::var(VAR).unwrap(), "permanent");

    env_set(VAR, "overwritten");
    assert_eq!(env::var(VAR).unwrap(), "overwritten");

    env_remove(VAR);
    assert!(env::var(VAR).is_err());
}

/// The double-hatch constructor flips both SSRF entry variables in one
/// atomic step and restores the pre-existing (absent) state on drop.
#[test]
fn ssrf_hatches_off_sets_both_and_restores_both() {
    use webfang_core::domain::ssrf_guard::{DISABLE_ENTRY_GUARD_ENV, WEBFANG_MCP_DISABLE_SSRF_ENV};
    {
        let _lock = env_lock();
        env::remove_var(WEBFANG_MCP_DISABLE_SSRF_ENV);
        env::remove_var(DISABLE_ENTRY_GUARD_ENV);
    }

    {
        let _guard = EnvGuard::ssrf_hatches_off();
        assert_eq!(env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).unwrap(), "1");
        assert_eq!(env::var(DISABLE_ENTRY_GUARD_ENV).unwrap(), "1");
    }

    assert!(env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).is_err());
    assert!(env::var(DISABLE_ENTRY_GUARD_ENV).is_err());
}

/// The #1396 boundary pin: `entry_guard_off` lifts the literal-IP entry
/// guard and NOTHING else. The three sibling hatches stay untouched, so a
/// suite that leans on this constructor cannot silently disarm the MCP
/// validator, the resolving-time DNS guard, or the redirect guard — the
/// failure mode #1308 was about. Restoration is on drop.
#[test]
fn entry_guard_off_arms_only_the_entry_hatch_and_restores_it() {
    use webfang_core::domain::ssrf_guard::{
        DISABLE_ENTRY_GUARD_ENV, DISABLE_REDIRECT_GUARD_ENV, DISABLE_VALIDATING_RESOLVER_ENV,
        WEBFANG_MCP_DISABLE_SSRF_ENV,
    };
    let hatches = [
        DISABLE_ENTRY_GUARD_ENV,
        WEBFANG_MCP_DISABLE_SSRF_ENV,
        DISABLE_VALIDATING_RESOLVER_ENV,
        DISABLE_REDIRECT_GUARD_ENV,
    ];
    {
        let _lock = env_lock();
        for hatch in hatches {
            env::remove_var(hatch);
        }
        // One sibling starts armed so "untouched" cannot be confused with
        // "set to nothing": drop must put the armed value back.
        env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");
    }

    {
        let _guard = EnvGuard::entry_guard_off();
        assert_eq!(
            env::var(DISABLE_ENTRY_GUARD_ENV).as_deref(),
            Ok("1"),
            "entry hatch must be armed with the exact value the guard reads"
        );
        assert!(
            env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).is_err(),
            "MCP validator hatch must stay armed (untouched)"
        );
        assert_eq!(
            env::var(DISABLE_VALIDATING_RESOLVER_ENV).as_deref(),
            Ok("1"),
            "resolver hatch must keep the value it had before the guard"
        );
        assert!(
            env::var(DISABLE_REDIRECT_GUARD_ENV).is_err(),
            "redirect hatch must stay armed (untouched)"
        );
    }

    {
        let _lock = env_lock();
        assert!(
            env::var(DISABLE_ENTRY_GUARD_ENV).is_err(),
            "entry hatch must be restored to its absent original"
        );
        assert_eq!(
            env::var(DISABLE_VALIDATING_RESOLVER_ENV).as_deref(),
            Ok("1"),
            "the sibling armed before the guard must survive it"
        );
        env::remove_var(DISABLE_VALIDATING_RESOLVER_ENV);
    }
}

/// #1788 acceptance pin: the latch must be restored after a **panic**
/// exit, not only on the happy-path drop.
///
/// Every other guard test drops at the end of a scope that exits normally,
/// so all of them exercise the same unwind. This one drives the guard out
/// of scope through `panic!` instead, which is the only way the restore
/// path actually runs in a real failing suite: a guard built at the top of
/// a test, a `#[tokio::test]` assertion blowing up mid-await, and the
/// process continuing with the next test.
///
/// The assertion also pins the poisoning consequence: the panic unwinds
/// through `Drop` *while the mutex guard is still held*, so `ENV_LOCK` is
/// left poisoned. The post-unwind read below only succeeds because every
/// acquisition tolerates poisoning — which is precisely the contract the
/// module docs state, and precisely what turns one failure into a cascade if
/// it is forgotten.
#[test]
fn entry_guard_restores_the_entry_hatch_after_a_panic_exit() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

    // Capture the baseline under the lock, so the post-unwind comparison
    // is against a value no sibling could have moved underneath us.
    let original = {
        let _lock = env_lock();
        env::var(DISABLE_ENTRY_GUARD_ENV).ok()
    };

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _guard = EnvGuard::entry_guard_off();
        assert_eq!(
            env::var(DISABLE_ENTRY_GUARD_ENV).as_deref(),
            Ok("1"),
            "the latch must be armed inside the guard, or this pin proves nothing"
        );
        panic!("deliberate panic inside the guard's scope (#1788)");
    }));

    assert!(
        outcome.is_err(),
        "the closure must have panicked, otherwise the restore-on-unwind \
         path was never taken and this test asserts nothing"
    );

    // Re-acquire the (now poisoned) lock to read: no other guard can be
    // mid-window while it is held, so this observes OUR restore and not a
    // sibling's.
    let observed = {
        let _lock = env_lock();
        env::var(DISABLE_ENTRY_GUARD_ENV).ok()
    };
    assert_eq!(
        observed, original,
        "{DISABLE_ENTRY_GUARD_ENV} was not restored after the panic exit; \
         the latch leaked and every later test would silently run with the \
         SSRF entry guard disarmed"
    );
}

/// #1788 triangulation for the observer side: `entry_guard_on` must
/// REFUSE to build while the hatch is armed, and must be constructible (and
/// mutate nothing) once it is restored. Without the refusal arm, a test
/// could hold a guard that proves nothing; without the success arm, the
/// constructor would be unusable.
///
/// The panic is captured rather than propagated so both arms run in one
/// test, and the leftover is cleaned up before the second arm.
/// # Why it does not call the constructor itself
///
/// `ENV_LOCK` is not reentrant, so arranging "the hatch is set" inside
/// this test necessarily means HOLDING the lock — and calling
/// `EnvGuard::entry_guard_on` from there would re-acquire it on the same
/// thread and self-deadlock, hanging the whole test binary instead of
/// failing an assertion. That is the exact hazard #1224 documents, and it
/// is not observable as a test failure: the process just stops.
///
/// So the refusal is pinned on `assert_entry_guard_armed`, the unit that
/// decides it and the only thing `entry_guard_on` adds on top of taking the
/// lock. The lock-acquisition half needs no separate pin: it is the same
/// two lines `EnvGuard::entry_guard_off` already runs, and the second arm
/// below exercises it on a real, live construction.
#[test]
fn entry_guard_on_refuses_a_disarmed_entry_guard() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

    {
        let _disarmed = EnvGuard::entry_guard_off();
        let refused = catch_unwind(AssertUnwindSafe(assert_entry_guard_armed));
        let Err(message) = refused else {
            panic!("the armed-posture assertion must refuse while the hatch is set");
        };
        let text = message
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| message.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(
            text.contains(DISABLE_ENTRY_GUARD_ENV) && text.contains("leaked the hatch"),
            "the diagnostic must name the variable and the leak, got: {text}"
        );
    }

    // Outside the disarm window the same constructor must build cleanly,
    // holding the lock without touching the environment.
    let before = {
        let _lock = env_lock();
        env::var(DISABLE_ENTRY_GUARD_ENV).ok()
    };
    {
        let _armed = EnvGuard::entry_guard_on();
        // Read directly: the guard already holds ENV_LOCK, so a second
        // acquisition here would self-deadlock (the #1224 invariant).
        assert_eq!(
            env::var(DISABLE_ENTRY_GUARD_ENV).ok(),
            before,
            "entry_guard_on must not mutate the environment"
        );
    }
}
