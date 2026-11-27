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
// consumers. Crate-internal helpers are reached through their module
// paths by the unit tests that pin them.
pub use bin_path::{webfang_path, PREBUILT_BIN_ENV};
pub use env::{env_lock, env_remove, env_set, EnvGuard};
pub use provider::test_provider_config;
pub use redaction::{redact_nondeterministic, redact_temp_path};

pub mod provider;

#[cfg(test)]
#[path = "bin_path_tests.rs"]
mod bin_path_tests;

#[cfg(test)]
#[path = "env_tests.rs"]
mod env_tests;

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod redaction_tests;
