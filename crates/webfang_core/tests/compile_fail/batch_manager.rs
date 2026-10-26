//! #2355 executable proof: batch construction rejects zero concurrency at
//! compile time.
//!
//! The case under `batch_manager/` calls [`BatchManager::new`] with
//! `max_concurrent == 0`. trybuild asserts it fails compilation — the
//! concurrency parameter is typed `NonZeroUsize`, so a zero argument is a
//! type error, never a runtime panic.

#[test]
fn zero_batch_concurrency_fails_to_compile() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/compile_fail/batch_manager/*.rs");
}
