#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
// Sanctioned owner of raw env mutations (#1126, #1349): every call here is
// ENV_LOCK-serialized and audited. The workspace clippy.toml configures
// `disallowed-methods` for std::env::set_var/remove_var, which fires in
// every crate once configured — this allow is the owner's exemption, the
// mirror image of the deny active in webfang_core and webfang_mcp.
#![allow(clippy::disallowed_methods)]
// Harness crate exempt from the workspace panic lints (#2354): this crate
// compiles OUTSIDE cfg(test) (its consumers are the tests), so the
// `cfg_attr(test, ...)` carve-out used at the other roots does not apply
// here. unwrap/expect on fallible setup steps is the harness idiom.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Shared test utilities for the webfang workspace.
//!
//! Provides RAII environment isolation, output redaction for deterministic
//! snapshots, and binary path resolution for integration tests.
//!
//! # SSRF test hatches — the ONE place they are documented (#1329)
//!
//! Test harnesses driving the production network path against wiremock
//! (127.0.0.1) must disarm the SSRF layers the path consults. Each hatch is
//! read with a distinct convention; all are test-only — production never
//! sets them:
//!
//! | Hatch | Canonical const | Layer it disarms |
//! |---|---|---|
//! | `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (exact `"1"`) | `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV` | Literal-IP entry guard (SSRF choke point, #1217) |
//! | `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD` | `webfang_core::domain::ssrf_guard::DISABLE_REDIRECT_GUARD_ENV` | Client redirect policy's literal-IP stop |
//! | `WEBFANG_DISABLE_SSRF_RESOLVER` | `webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV` | Connect-time validating DNS resolver |
//! | `WEBFANG_DISABLE_SSRF` (presence) | `webfang_core::domain::ssrf_guard::LLM_SSRF_TEST_HATCH_ENV` | LLM base-URL SSRF gate |
//! | `WEBFANG_MCP_DISABLE_SSRF` (exact `"1"`) | `webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV` (#1348) | MCP entry validator |
//!
//! `WEBFANG_MCP_DISABLE_SSRF` disables the MCP layer only for the exact value
//! `"1"`; every other value leaves it enabled. This does not change the
//! separate `WEBFANG_DISABLE_SSRF` contract: that variable remains
//! presence-based for the LLM extraction base-URL SSRF gate.
//!
//! #1615 DF-E9: `WEBFANG_DISABLE_SSRF` is no longer a PRODUCTION hatch. Its
//! read is behind `#[cfg(test)]`, so it disarms the LLM gate only inside this
//! crate's own unit tests and cannot disarm it in a deployed process at all.
//! That is the one row in the table above whose scope is narrower than its
//! "all are test-only — production never sets them" preamble suggests: the
//! other hatches are production code that a test may set; this one does not
//! exist in production code. An integration test under `crates/*/tests/` links
//! `webfang_core` externally and therefore does not see `cfg(test)`, so it
//! cannot arm this row — it must not need to, and none does.
//!
//! # Which hatch may a test arm? (#1308, scoped by #1396)
//!
//! The #1308 rule is about the **robots chain**, whose two entry hatches —
//! `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` and `WEBFANG_MCP_DISABLE_SSRF` — are
//! read by the same fetch path. A robots test that arms only one of them
//! leaves the other layer armed, so it can pass on a phantom denial label
//! instead of the robots rules it means to exercise. For that chain the rule
//! is still absolute: use [`EnvGuard::wiremock_robots`] and never arm one of
//! *its two* hatches by hand.
//!
//! It was never a ban on single-hatch tests, and #1396 says so explicitly.
//! A suite whose subject is one layer arms exactly that layer — through a
//! named constructor, never by spelling the variable out at the call site:
//!
//! | Path under test | Constructor | Hatches armed |
//! |---|---|---|
//! | robots chain | [`EnvGuard::wiremock_robots`] | entry guard + MCP validator |
//! | MCP scrape of a loopback mock | [`EnvGuard::ssrf_hatches_off`] | entry guard + MCP validator |
//! | entry guard only (sitemap parse/discover suites) | [`EnvGuard::entry_guard_off`] | entry guard only |
//! | entry guard as the SUBJECT (it must stay armed and fire) | [`EnvGuard::entry_guard_on`] | none — asserts armed |
//! | entry guard as the subject, on a loopback seed that needs layer 3 lifted | [`EnvGuard::entry_guard_on_resolver_off`] | validating resolver only; asserts entry guard armed |
//!
//! # A mutex only protects the threads that take it (#1788)
//!
//! Everything above frames `ENV_LOCK` as protecting **mutation**, and it is
//! exactly that: no two guard constructors run their `env::set_var` at the
//! same time, so a write window is never observed torn. That guarantee,
//! however, is conditional on the *reader* taking the lock too — and a reader
//! who does not take it is not protected at all.
//!
//! A test that needs the environment in its **default / armed posture** is
//! exactly such a reader. It mutates nothing, so nothing warns it that the
//! shared state it reads is mutable. Meanwhile a sibling thread may already be
//! inside an `EnvGuard::entry_guard_off()` window with the hatch set, and the
//! observer's assertions then describe the *sibling's* state, not its own.
//!
//! Under nextest this is nearly invisible: every test is its own process, so
//! the only other holder of `ENV_LOCK` would have to be in that very process,
//! which nothing else is. Under **libtest** it is a real race — every `--lib`
//! test shares one process across N threads, and the `Coverage` job runs
//! exactly that (no `--nextest`, #1788). There the four sitemap-discovery pins
//! that assert an *armed* entry guard (`..._rejects_loopback_seed_pre_socket`
//! and its three siblings) intermittently observed a disarmed guard, let an
//! RFC1918 literal through, and failed with `Http { Connect }` where the
//! contract says `CrawlError::InvalidUrl`.
//!
//! **The rule:** a test that asserts the shared state is in its default or
//! armed posture must take the same lock as one that changes it — otherwise it
//! is not protected at all. [`EnvGuard::entry_guard_on`] is that
//! lock-acquiring observer for the SSRF entry guard: it holds `ENV_LOCK` for
//! its whole lifetime, changes nothing, and asserts the hatch is not set to
//! the exact `"1"` the guard reads. Excluding the disarm window is a
//! precondition of observing the armed state, not a side effect of wanting to
//! change it.
//!
//! # Poisoning stays tolerated (#1788)
//!
//! A guard holds the lock across arbitrary test code, so any test that
//! panics while holding it poisons `ENV_LOCK` for every later test in the
//! process. Every acquisition in this module therefore uses
//! `unwrap_or_else(|poisoned| poisoned.into_inner())`: the environment is the
//! thing being serialized, and it is restored by `Drop` during the unwind
//! regardless of the mutex's poisoned flag, so refusing the lock afterwards
//! would cascade one failure into the whole suite. `EnvGuard::entry_guard_on`
//! follows the same rule — including the `catch_unwind` pin below, which
//! proves the restore-on-unwind path rather than the happy-path drop.
//!
//! What #1308 actually forbids is an *ad-hoc* hatch: a literal env name at a
//! call site, where a rename silently desynchronizes writer and reader and
//! where nobody can see which layer the test disarmed. Adding another named
//! constructor is cheap; restating a variable is not.
//!
//! # Nesting invariant: prime env-writing `Once` init BEFORE taking `ENV_LOCK` (#1224)
//!
//! Every constructor and helper in this module acquires the process-wide
//! `ENV_LOCK`, and that lock is **not reentrant**. An `EnvGuard` holds it for
//! its entire lifetime, so the ordering of *process-wide lazy
//! initialization* is load-bearing — getting it wrong self-deadlocked the
//! whole `Tests (all features)` lane (PR #1224):
//!
//! - Init that only **reads** the env, or mutates none, is safe anywhere —
//!   e.g. an `OnceLock` that installs a port (`ensure_waf_inspector` in
//!   `application/crawler/sitemap_discovery.rs`).
//! - Init that **writes** the env — the shape is
//!   `Once::call_once(|| env_set(..))` or `OnceLock::get_or_init(|| env_set(..))`,
//!   because [`env_set`] and [`env_remove`] take `ENV_LOCK` themselves — run
//!   for the first time *inside* a guard's scope deadlocks: the init blocks on
//!   a lock the same test already holds. Under nextest every test is its own
//!   process, so the `Once` has genuinely not fired yet and the hang is total.
//!
//! The fix is to prime the `Once` while this task holds no lock, in a blocking
//! thread, and only then build the guard (live instance: `ssrf_guards_off` in
//! `crates/webfang_mcp/tests/scraping_coverage_test.rs`):
//!
//! ```ignore
//! // Prime the ONCE outside our own lock scope (#1224).
//! let _ = tokio::task::spawn_blocking(init_ssrf_disabled).await;
//! let _guards = webfang_test_utils::EnvGuard::entry_guard_off();
//! ```
//!
//! Keep the idempotent init at its original call site as well — after priming
//! it is a no-op, and it still covers harnesses that run without a guard.

/// OS-dependent connection-failure collapse used by the shared redactor.
mod redaction_network;

mod bin_path;
mod env;
mod redaction;

// The crate root is the ONLY public surface: every historical
// `webfang_test_utils::<item>` path keeps resolving through these
// re-exports, so the env/redaction/bin_path split is invisible to
// consumers. Crate-internal helpers the sibling test files exercise stay
// reachable through the `pub(crate) use` re-exports.
pub use bin_path::{webfang_path, PREBUILT_BIN_ENV};
pub use env::{env_lock, env_remove, env_set, EnvGuard};
pub use redaction::{redact_nondeterministic, redact_temp_path};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bin_path::{
        prebuilt_bin_declared, resolve_webfang_path, workspace_root_from_manifest,
    };
    use crate::env::assert_entry_guard_armed;
    use std::env;
    use std::path::{Path, PathBuf};

    /// The expected binary leaf name is platform-dependent: Windows builds
    /// `webfang.exe`. Mirrors `resolve_webfang_path`'s own
    /// `set_extension("exe")` so the expected constants stay cross-platform.
    fn expected_binary_name() -> &'static str {
        if cfg!(windows) {
            "webfang.exe"
        } else {
            "webfang"
        }
    }

    /// #2242 regression pin: the token-anchored separator rule must cover the
    /// WHOLE path run that follows the token, not only the first separator.
    /// The platform default config path nests one level (`<base>/webfang/
    /// config.toml`), so Windows renders inner `\` separators that a
    /// single-separator rewrite left in the snapshot.
    #[test]
    fn redaction_normalizes_every_separator_after_the_token() {
        let dir = Path::new("C:\\Users\\runner\\tmp\\base");
        let text =
            "el archivo de configuración C:\\Users\\runner\\tmp\\base\\webfang\\config.toml \
                    no es TOML válido";
        let redacted = redact_nondeterministic(dir, text);
        assert!(
            redacted.contains("<OUT_DIR>/webfang/config.toml"),
            "every separator after the token must normalize: {redacted}"
        );
    }

    /// #1366 regression pin: with BOTH harness variables absent
    /// (`CARGO_BIN_EXE_webfang` unset — the binary belongs to a sibling crate —
    /// and `CARGO_TARGET_DIR` unset — a runner without direnv), the fallback
    /// must resolve the binary under the workspace root's own `target/`,
    /// reached by climbing exactly TWO parent hops from the crate manifest.
    /// The bug climbed three, landing one directory too high (the repo's
    /// parent), so the fallback path pointed at a `target/` directory cargo
    /// never populates — the build succeeded and the returned path was still
    /// wrong. This test simulates the absent-variable environment by passing
    /// `None` explicitly: it never mutates (or even reads) the process
    /// environment, so it cannot race ENV_LOCK or leak state into siblings.
    #[test]
    fn resolve_webfang_path_without_both_env_vars_uses_workspace_target() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let path = resolve_webfang_path(None, None, manifest);

        let root = workspace_root_from_manifest(manifest);
        assert_eq!(
            path,
            root.join("target")
                .join("debug")
                .join(expected_binary_name())
        );

        // The two-hop root must be the real workspace root: it holds the
        // virtual manifest, and it is NOT the three-hop directory the #1366
        // bug used to land in (the repo's parent).
        assert!(root.join("Cargo.toml").is_file());
        let buggy_three_hop_root = Path::new(manifest)
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("three-hop chain resolves");
        assert_ne!(root, buggy_three_hop_root);
    }

    /// Cross-check the two-hop climb against the authoritative source of
    /// truth: `cargo locate-project --workspace` reports exactly where the
    /// workspace manifest lives. #1366 regressed here because the climb was
    /// hand-rolled; this pin fails if the climb and cargo ever disagree again.
    #[test]
    fn workspace_root_from_manifest_matches_cargo_metadata() {
        let root = workspace_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let out = std::process::Command::new(option_env!("CARGO").unwrap_or("cargo"))
            .args([
                "locate-project",
                "--workspace",
                "--message-format=plain",
                "--manifest-path",
            ])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .output()
            .expect("spawn cargo locate-project");
        assert!(out.status.success(), "cargo locate-project failed");
        // locate-project prints the workspace manifest path; strip the
        // `Cargo.toml` leaf to compare roots.
        let manifest_out = String::from_utf8(out.stdout).expect("locate-project prints UTF-8");
        let cargo_root = Path::new(manifest_out.trim_end())
            .parent()
            .expect("locate-project prints a manifest path");
        assert_eq!(
            root, cargo_root,
            "two-hop climb must land on cargo's own workspace root"
        );
    }

    /// Behavioral pins for the variable-present branches — the paths that
    /// must NOT change with #1366: an explicit `CARGO_BIN_EXE_webfang` wins
    /// outright, and an explicit `CARGO_TARGET_DIR` roots the debug output.
    #[test]
    fn resolve_webfang_path_respects_explicit_env_vars() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        assert_eq!(
            resolve_webfang_path(Some("/explicit/webfang"), None, manifest),
            PathBuf::from("/explicit/webfang")
        );

        let via_target = resolve_webfang_path(None, Some("/custom/target"), manifest);
        assert_eq!(
            via_target,
            PathBuf::from("/custom/target/debug").join(expected_binary_name())
        );
    }

    /// The runner contract is an exact marker: only `"1"` declares a
    /// pre-built binary. Any other value — including an empty string, which a
    /// sloppy workflow could interpolate — keeps the local on-demand build, so
    /// a typo never turns into a silent stale-binary reuse (#2331).
    #[test]
    fn only_the_exact_marker_declares_a_prebuilt_binary() {
        assert!(prebuilt_bin_declared(Some("1")));
        assert!(!prebuilt_bin_declared(Some("0")));
        assert!(!prebuilt_bin_declared(Some("true")));
        assert!(!prebuilt_bin_declared(Some("")));
        assert!(!prebuilt_bin_declared(None));
    }

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
        use webfang_core::domain::ssrf_guard::{
            DISABLE_ENTRY_GUARD_ENV, WEBFANG_MCP_DISABLE_SSRF_ENV,
        };
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

    #[test]
    fn redact_nondeterministic_normalizes_timestamps() {
        let dir = Path::new("/tmp/test");
        let input = "at 2024-03-15T10:30:00.123+01:00 done";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "at <TIMESTAMP> done");
    }

    #[test]
    fn redact_nondeterministic_normalizes_ports() {
        let dir = Path::new("/tmp/test");
        let input = "server at 127.0.0.1:8080 started";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "server at 127.0.0.1:<PORT> started");
    }

    #[test]
    fn redact_nondeterministic_strips_ansi() {
        let dir = Path::new("/tmp/test");
        let input = "\x1b[31merror\x1b[0m: something failed";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error: something failed");
    }

    #[test]
    fn redact_nondeterministic_replaces_temp_path() {
        let dir = Path::new("/tmp/.tmpABC123");
        let input = "wrote /tmp/.tmpABC123/output.md";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "wrote <OUT_DIR>/output.md");
    }

    /// #1777: the redaction token stands in for a directory PREFIX, but the
    /// separator that FOLLOWED the prefix is the OS's. A message carrying a
    /// real Windows path renders `<OUT_DIR>\typo.toml` where POSIX renders
    /// `<OUT_DIR>/typo.toml`, so a committed snapshot can only ever match one
    /// of them. `override_is_relative` passes on both platforms precisely
    /// because its message carries no separator.
    #[test]
    fn redact_nondeterministic_normalizes_the_separator_after_a_path_token() {
        let dir = Path::new("/tmp/test");
        let input = "no existe el archivo de configuración: <OUT_DIR>\\typo.toml";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(
            result,
            "no existe el archivo de configuración: <OUT_DIR>/typo.toml"
        );
    }

    /// The counterpart claim: a backslash that is NOT adjacent to a redaction
    /// token is content, not a separator, and must survive untouched. Without
    /// this the rule above could be widened to "replace every backslash",
    /// which would silently corrupt any snapshot whose message happens to
    /// contain one (a Windows-style escape, a regex, a literal path).
    #[test]
    fn redact_nondeterministic_leaves_a_backslash_that_is_not_after_a_token() {
        let dir = Path::new("/tmp/test");
        let input = r"patrón \N no coincide y <OUT_DIR>/ok.toml sí";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, r"patrón \N no coincide y <OUT_DIR>/ok.toml sí");
    }

    #[test]
    fn redact_nondeterministic_normalizes_line_numbers() {
        let dir = Path::new("/tmp/test");
        let input = "see scrape_flow.rs:193 for details";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "see scrape_flow.rs:<LINE> for details");
    }

    #[test]
    fn redact_nondeterministic_normalizes_tracing_module_paths() {
        let dir = Path::new("/tmp/test");
        let input =
            "  2024-03-15T10:30:00+01:00  WARN webfang_core::cli::orchestrator: Unknown profile";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "  <TIMESTAMP>  WARN <MODULE>: Unknown profile");
    }

    /// #688 pin: `log_scrape_error` stamps a `trace_id` UUID on every error
    /// event, so any snapshot of stderr would bake a fresh UUID per run
    /// without this rule. The rule lives here (the shared chain) since
    /// #1649, so EVERY consumer redacts it — previously only
    /// `webfang_core`'s private harness copy had it, and this distributed
    /// copy did not.
    #[test]
    fn redact_nondeterministic_normalizes_trace_ids() {
        let dir = Path::new("/tmp/test");
        let input =
            "error de red: failed trace_id=3f2504e0-4f89-11d3-9a0c-0305e82c3301 correlation=ABCDEF01-2345-6789-ABCD-EF0123456789";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(
            result,
            "error de red: failed trace_id=<TRACE_ID> correlation=<TRACE_ID>"
        );
    }

    #[test]
    fn redact_nondeterministic_normalizes_tracing_file_paths() {
        let dir = Path::new("/tmp/test");
        let input = "    at crates/webfang_core/src/cli/orchestrator.rs:42";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "    at <FILE>.rs:<LINE>");
    }

    /// INT-1 (#1631) pin, Unix shape: the errno is OS text (`os error 111`
    /// on Linux, `61` on macOS) and a snapshot must not bake it in. The
    /// surrounding Spanish error prefix is product-owned and survives; the
    /// `I/O error: ` layer does NOT, because the Windows timeout path has no
    /// such layer and keeping it made the two platforms disagree — see
    /// `redact_nondeterministic_makes_both_platforms_agree`.
    #[test]
    fn redact_nondeterministic_collapses_unix_connection_refused() {
        let dir = Path::new("/tmp/test");
        let input = "error de red: I/O error: Connection refused (os error 111)";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error de red: <NET_ERR>");
    }

    /// INT-1 (#1631) pin, Windows shape: the refusal is WSA prose plus the
    /// same errno parenthetical, so the WHOLE phrase must collapse — a
    /// narrower rule that only replaced the number would still leave
    /// Windows-only text in a Linux-authored snapshot.
    #[test]
    fn redact_nondeterministic_collapses_windows_wsa_prose() {
        let dir = Path::new("/tmp/test");
        let input = "No connection could be made because the target machine actively refused it. (os error 10061)";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "<NET_ERR>");
    }

    /// INT-1 (#1631) pin, Windows timeout-degradation shape: an unreachable
    /// host on Windows never refuses, it times out, so the snapshot has to
    /// see the SAME token the Unix refusal produces.
    #[test]
    fn redact_nondeterministic_collapses_windows_timeout_degradation() {
        let dir = Path::new("/tmp/test");
        let input = "error de red: request timed out after 2s";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error de red: <NET_ERR>");
    }

    /// The pin that would have caught the `I/O error: ` layer the first two
    /// pins missed. Both strings below are copied BYTE-EXACT from the stderr
    /// each platform produced for `unreachable_host_stderr_mentions_failure`
    /// (Windows lane, run 36356048341) — not reconstructed from memory:
    ///
    /// ```text
    /// linux:   error de red: I/O error: Connection refused (os error 111)
    /// windows: error de red: request timed out after 2s
    /// ```
    ///
    /// Written as two separate pins, they both passed while the RULE was wrong,
    /// because each pin asserted only its own shape: the Unix pin expected the
    /// `I/O error: ` layer to SURVIVE, and the Windows pin had no layer to
    /// begin with. The defect was only visible when the two are compared, so
    /// this asserts the comparison itself — the whole point of the collapse.
    #[test]
    fn redact_nondeterministic_makes_both_platforms_agree() {
        let dir = Path::new("/tmp/test");
        let unix = redact_nondeterministic(
            dir,
            "Failed to scrape http://x/: error de red: I/O error: Connection refused (os error 111)",
        );
        let windows = redact_nondeterministic(
            dir,
            "Failed to scrape http://x/: error de red: request timed out after 2s",
        );
        assert_eq!(unix, windows, "the two platforms must redact identically");
        assert_eq!(
            unix, "Failed to scrape http://x/: error de red: <NET_ERR>",
            "and both must land on the documented token"
        );
    }
}
