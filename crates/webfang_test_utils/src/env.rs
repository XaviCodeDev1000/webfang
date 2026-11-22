//! Process-environment isolation for tests: the ENV_LOCK owner and the
//! [`EnvGuard`] RAII guard with its SSRF-hatch constructors.
//!
//! The workspace contracts that govern this module (the hatch table, the
//! #1308/#1396 scoping, the #1788 armed-posture observer and the #1224
//! nesting invariant) are documented at the crate root: they span every
//! consumer of this harness, not only the guard itself.

use std::env;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the process-wide environment lock.
///
/// Workspace invariant (issue #1126): **every** mutation of the process
/// environment must hold `ENV_LOCK`, because concurrent test threads share
/// one environment and `env::set_var`/`env::remove_var` race with readers.
/// Prefer [`EnvGuard`], which acquires the lock, mutates, and restores on
/// drop. Use `env_lock` directly only when the guard's restore-on-drop
/// semantics do not fit — e.g. seeding state before a guard exists, or a
/// one-time process-wide cleanup — and keep the mutation inside the scope
/// of the returned guard.
///
/// The lock is reentrant-hostile: `EnvGuard` constructors and methods also
/// acquire it, so never call `env_lock` while an `EnvGuard` is alive.
#[must_use = "the returned guard releases the lock when dropped; bind it to a name that outlives the mutation"]
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Set `var` to `value` under `ENV_LOCK` with **permanent** semantics: the
/// change is NOT restored on drop — it stays for the rest of the process.
///
/// Use for one-time/permanent seeding or cleanup where restore-on-drop does
/// not fit (#1126) — e.g. harness initialization inside `Once::call_once`.
/// For atomic multi-variable setup that must restore, use
/// [`EnvGuard::with`] / [`EnvGuard::clean`]; to flip a variable mid-test
/// while already holding a guard, use [`EnvGuard::set`] /
/// [`EnvGuard::remove`].
///
/// NEVER call while an [`env_lock()`] guard or an [`EnvGuard`] scope is
/// alive: this helper acquires `ENV_LOCK` itself, and the lock is not
/// reentrant (deadlock). This is exactly why it replaces the old
/// `let _lock = env_lock(); std::env::set_var(...)` pattern — the manual
/// binding disappears, the serialization does not.
#[allow(unsafe_code)]
pub fn env_set(var: &str, value: &str) {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
    // access the environment while this lock is held.
    unsafe {
        env::set_var(var, value);
    }
}

/// Remove `var` under `ENV_LOCK` with **permanent** semantics: the variable
/// is NOT restored on drop — it stays absent for the rest of the process.
///
/// Same contract as [`env_set`]: one-time/permanent cleanup where
/// restore-on-drop does not fit (#1126); [`EnvGuard`] variants for scoped or
/// mid-test changes; never call while an [`env_lock()`] guard or an
/// [`EnvGuard`] scope is alive (the helper acquires `ENV_LOCK` itself, and
/// the lock is not reentrant).
#[allow(unsafe_code)]
pub fn env_remove(var: &str) {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
    // access the environment while this lock is held.
    unsafe {
        env::remove_var(var);
    }
}

/// RAII guard that isolates environment variable mutations in tests.
///
/// Acquires the global [`env_lock`] on construction and restores all
/// modified variables to their original state on drop. This guarantees
/// serial access to the process environment across concurrent test threads.
/// Mutations that must happen while the guard is already held (flipping a
/// flag mid-test, stepping through values) go through [`EnvGuard::set`] and
/// [`EnvGuard::remove`] — never a raw `env::set_var`, which would bypass
/// the serialization the guard exists to provide.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    original_vars: Vec<(String, Option<String>)>,
}

// SAFETY: `env::set_var` / `env::remove_var` are only unsound when racing.
// ENV_LOCK serializes every process-environment mutation performed through
// this guard, and the guard restores the original state on drop.
#[allow(unsafe_code)]
impl EnvGuard {
    /// Arm BOTH SSRF hatches the robots chain can consult in tests (#1329):
    ///
    /// 1. `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` — core's literal-IP entry guard
    ///    (canonical const:
    ///    `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`), read
    ///    once per chain inside `RobotsFetcher` and again at CLI/MCP entry
    ///    points;
    /// 2. `WEBFANG_MCP_DISABLE_SSRF` — the MCP validator's hatch (canonical
    ///    const:
    ///    `webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV`,
    ///    landed with #1348 and adopted here with #1370).
    ///
    /// The #1308 lesson: a robots test that arms only ONE of these two leaves
    /// the other chain layer armed, so the test can pass on a phantom denial
    /// label instead of the robots rules it means to exercise. Always use this
    /// constructor for tests that drive the robots path against a wiremock
    /// loopback literal — within that chain, never arm one of its two hatches
    /// on its own.
    ///
    /// The rule is scoped to the robots chain, not to every test (#1396): a
    /// suite whose subject is a single different layer disarms exactly that
    /// layer, through its own named constructor — [`EnvGuard::entry_guard_off`]
    /// for the literal-IP entry guard, [`EnvGuard::ssrf_hatches_off`] for an MCP
    /// loopback scrape. What stays forbidden everywhere is the ad-hoc form: a
    /// literal env variable spelled out at the call site.
    ///
    /// The guard restores both variables on drop.
    #[must_use]
    pub fn wiremock_robots() -> Self {
        Self::with(&[
            (
                webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                "1",
            ),
            (
                webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
                "1",
            ),
        ])
    }

    /// Disarm ONLY the literal-IP SSRF entry guard (#1396).
    ///
    /// Sets `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` to the exact `"1"` the guard
    /// demands (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) and nothing
    /// else: the MCP validator, the connect-time validating resolver (layer 3)
    /// and the redirect guard (layer 4) all stay armed. The guard restores the
    /// variable on drop.
    ///
    /// Use it for suites whose subject is a path that trips over exactly one
    /// layer — a wiremock loopback driven through the sitemap parser or the
    /// discovery chain (#1382): the entry guard rejects the literal seed, the
    /// other layers never see it. It is NOT the right helper for the robots
    /// chain, which consults two hatches: use [`EnvGuard::wiremock_robots`]
    /// there (#1308, see the module docs).
    ///
    /// This replaces five byte-identical local `fn entry_guard_off` helpers that
    /// had drifted into five copies of the same env name and value. The posture
    /// argument ("this suite pins X, not the guard") is per-file and stays at
    /// the call site as a comment; the mutation itself lives here, once.
    ///
    /// # Nesting (#1224)
    ///
    /// The returned guard holds the non-reentrant `ENV_LOCK` for its whole
    /// lifetime. If the harness has a process-wide `Once` init that *writes* the
    /// environment, prime it in `spawn_blocking` before calling this, or the
    /// first fetch self-deadlocks on a lock this test already holds — see the
    /// module-doc section "Nesting invariant".
    #[must_use]
    pub fn entry_guard_off() -> Self {
        Self::with(&[(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        )])
    }

    /// Hold `ENV_LOCK` for this guard's whole lifetime and assert the
    /// literal-IP SSRF entry guard is **ARMED**, mutating nothing (#1788).
    ///
    /// The mirror image of [`EnvGuard::entry_guard_off`]: where that one
    /// disarms layer 1 and restores it on drop, this one refuses to run at all
    /// if layer 1 is currently disarmed, and changes no variable. The returned
    /// guard holds the lock until it is dropped, so no sibling test can open a
    /// disarm window for the duration of the caller's assertions.
    ///
    /// # Why
    ///
    /// `ENV_LOCK` serializes only the threads that TAKE it. The module docs
    /// frame it as protecting mutation, which is true and is not the hazard
    /// here: a test that must *observe* the armed posture mutates nothing, so
    /// nothing prompts it to take the lock — and without the lock it observes
    /// whatever a sibling thread's `entry_guard_off()` window happens to hold.
    /// The exclusion window is a precondition of observing the armed state, so
    /// an observer needs the same lock as a mutator; taking it is the whole
    /// point of this constructor.
    ///
    /// Only the exact `"1"` counts as disarmed, mirroring
    /// `reject_forbidden_literal_url`'s own read — any other value leaves the
    /// guard armed, and this constructor accepts it.
    ///
    /// # Panics
    ///
    /// Panics if `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) is set to
    /// the exact `"1"` while this guard is being built. The message names the
    /// variable, its observed value, and the fact that another test leaked the
    /// hatch.
    ///
    /// # Nesting (#1224, #1788)
    ///
    /// `ENV_LOCK` is **not reentrant**, and this guard holds it for its whole
    /// lifetime: never combine it with another `EnvGuard` in the same scope, or
    /// the test self-deadlocks. A test that needs a hatch lifted *as well*
    /// must use the single-step [`EnvGuard::entry_guard_on_resolver_off`]
    /// instead of stacking two guards.
    #[must_use]
    pub fn entry_guard_on() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_entry_guard_armed();
        Self {
            _lock: lock,
            original_vars: Vec::new(),
        }
    }

    /// Keep the literal-IP entry guard ARMED (asserted) while lifting ONLY the
    /// connect-time validating resolver hatch (#1788).
    ///
    /// The posture a loopback-seed harness needs when layer 1 is the SUBJECT:
    /// the seed is spelled as a hostname (`localhost`) so layer 3 would reject
    /// the 127.0.0.1 it resolves to, and layer 1 must stay armed so a
    /// forbidden *literal* elsewhere in the chain is what fires.
    ///
    /// # Why one constructor and not two guards
    ///
    /// `ENV_LOCK` is not reentrant, so a test cannot hold both an
    /// `entry_guard_on()` observer and an `EnvGuard::with(..)` mutator: the
    /// second acquisition self-deadlocks. Folding both into one constructor is
    /// also the only shape that keeps the module's single-writer discipline
    /// (#1396): the resolver hatch's canonical name stays here, in the one
    /// place that is allowed to write it, instead of being restated at a call
    /// site where a rename would silently desynchronize writer and reader.
    /// Every other hatch stays armed — in particular the entry guard, which is
    /// asserted armed both before and after the writes below.
    ///
    /// # Panics
    ///
    /// Panics if `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) is set to
    /// the exact `"1"` either when this guard is built or after the resolver
    /// hatch is written — the second check rejects a caller that tries to smuggle
    /// the entry hatch in through another variable. On that panic the guard has
    /// already been constructed, so `Drop` still restores the resolver hatch
    /// during the unwind.
    ///
    /// # Nesting (#1224)
    ///
    /// The returned guard holds the non-reentrant `ENV_LOCK` for its whole
    /// lifetime — prime any env-*writing* process-wide `Once` in
    /// `spawn_blocking` first, exactly as for [`EnvGuard::entry_guard_off`].
    #[must_use]
    pub fn entry_guard_on_resolver_off() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_entry_guard_armed();
        let original =
            env::var(webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV).ok();
        // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
        // access the environment while this guard lives.
        unsafe {
            env::set_var(
                webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
                "1",
            );
        }
        // Build the guard BEFORE re-asserting, so a panic here unwinds through
        // `Drop` and restores the resolver hatch instead of leaking it.
        let guard = Self {
            _lock: lock,
            original_vars: vec![(
                webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV.to_owned(),
                original,
            )],
        };
        assert_entry_guard_armed();
        guard
    }

    /// Remove the given variables from the environment, saving originals for
    /// restoration on drop.
    #[must_use]
    pub fn clean(vars: &[&str]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut original_vars = Vec::with_capacity(vars.len());
        for &var in vars {
            let original = env::var(var).ok();
            original_vars.push((var.to_owned(), original));
            // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
            // access the environment while this guard lives.
            unsafe {
                env::remove_var(var);
            }
        }
        Self {
            _lock: lock,
            original_vars,
        }
    }

    /// Set the given variables in the environment, saving originals for
    /// restoration on drop.
    #[must_use]
    pub fn with(vars: &[(&str, &str)]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut original_vars = Vec::with_capacity(vars.len());
        for &(var, val) in vars {
            let original = env::var(var).ok();
            original_vars.push((var.to_owned(), original));
            // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
            // access the environment while this guard lives.
            unsafe {
                env::set_var(var, val);
            }
        }
        Self {
            _lock: lock,
            original_vars,
        }
    }

    /// Lift BOTH SSRF entry hatches for an MCP wiremock-loopback harness,
    /// restoring both on drop.
    ///
    /// The double-hatch helper (#1329 paso 4, #1348): an MCP harness that
    /// scrapes a loopback wiremock literal needs BOTH
    /// `WEBFANG_MCP_DISABLE_SSRF=1` (MCP layer 1, the DNS pre-check) AND
    /// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` (core layer 2, the literal-IP
    /// entry guard) — see docs/ssrf-layers.md. This constructor sets both in
    /// one atomic step referencing the canonical constants in
    /// `webfang_core::domain::ssrf_guard`. Layers 3 (validating resolver) and
    /// 4 (redirect guard) stay armed.
    #[must_use]
    pub fn ssrf_hatches_off() -> Self {
        Self::with(&[
            (
                webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
                "1",
            ),
            (
                webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                "1",
            ),
        ])
    }

    /// Set a variable while this guard already holds the environment lock,
    /// recording its current value for restoration on drop.
    ///
    /// Use this instead of a raw `env::set_var` when a test must flip a
    /// variable after the guard exists (e.g. proving a flag is captured at
    /// construction). The lock is already held by `self`, so this cannot
    /// deadlock and cannot race with any other environment mutation.
    pub fn set(&mut self, var: &str, value: &str) {
        let original = env::var(var).ok();
        self.original_vars.push((var.to_owned(), original));
        // SAFETY: ENV_LOCK is held by `self._lock` for the whole lifetime of
        // this guard, so no other thread can access the environment here.
        unsafe {
            env::set_var(var, value);
        }
    }

    /// Remove a variable while this guard already holds the environment
    /// lock, recording its current value for restoration on drop.
    ///
    /// Same serialization guarantee as [`EnvGuard::set`].
    pub fn remove(&mut self, var: &str) {
        let original = env::var(var).ok();
        self.original_vars.push((var.to_owned(), original));
        // SAFETY: ENV_LOCK is held by `self._lock` for the whole lifetime of
        // this guard, so no other thread can access the environment here.
        unsafe {
            env::remove_var(var);
        }
    }
}

/// Assert the literal-IP SSRF entry guard is ARMED, i.e. that its hatch
/// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` is not set to the exact `"1"` the
/// production read demands (#1788).
///
/// Caller contract: `ENV_LOCK` must already be held. The read is therefore
/// not itself the serialized part — the *exclusion* is. A caller that holds no
/// lock can read the hatch at any instant, including inside another test's
/// disarm window, which is the whole defect this assertion exists to surface.
pub(crate) fn assert_entry_guard_armed() {
    let var = webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;
    let observed = env::var(var).ok();
    assert!(
        observed.as_deref() != Some("1"),
        "{var} is set to \"1\" (observed: {observed:?}), so the SSRF entry guard is \
         DISARMED — another test leaked the hatch outside its own scope, or its \
         EnvGuard is still alive. A test whose subject IS the armed entry guard \
         must call EnvGuard::entry_guard_on() (or EnvGuard::entry_guard_on_resolver_off()) \
         so ENV_LOCK excludes the disarm window; taking no lock observes whichever \
         state a sibling thread happened to leave behind."
    );
}

// SAFETY: same ENV_LOCK serialization as the constructors; drop restores
// each variable to its captured original value (or removes it if unset).
// Restoration runs in reverse capture order so that when one variable is
// captured more than once (constructor plus `set`/`remove`), the earliest —
// true original — value is written last and wins.
#[allow(unsafe_code)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (var, original) in self.original_vars.iter().rev() {
            // SAFETY: ENV_LOCK exclusivity is guaranteed — the lock is held by
            // `self._lock` until this drop completes.
            unsafe {
                match original {
                    Some(val) => env::set_var(var, val),
                    None => env::remove_var(var),
                }
            }
        }
    }
}
