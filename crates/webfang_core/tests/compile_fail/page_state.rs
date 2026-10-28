//! Compile-fail executable proofs: illegal API usage does NOT compile.
//!
//! Both suites share ONE trybuild project (it is keyed by the crate under
//! test, not by the runner), so they must run from a single `#[test]`: two
//! parallel runners contend on the same cargo build lock, the lock wait
//! burns each runner's slow-timeout ceiling, and `flaky-result = "fail"`
//! turns the second finisher red (#2366, Windows cold run).
//!
//! - `page_state/` — each case attempts one illegal lifecycle move (state
//!   skip, non-recovery backward transition, COMMITTED escape, or
//!   self-transition). These moves have no method in the [`Stateful`]
//!   typestate API, so failure is a type error, never a runtime check.
//! - `batch_manager/` — `BatchManager::new(0)` must not compile: the
//!   concurrency parameter is a `NonZeroUsize`, so a zero argument is
//!   rejected by the type system instead of panicking at construction
//!   (#2355).

#[test]
fn compile_fail_cases_fail_to_compile() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/compile_fail/page_state/*.rs");
    cases.compile_fail("tests/compile_fail/batch_manager/*.rs");
}
