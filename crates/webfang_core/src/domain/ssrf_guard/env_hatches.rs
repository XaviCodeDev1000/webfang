//! Environment hatch reads, confined to unit-test builds (S9).
//!
//! A shipped binary cannot disarm the redirect stop or the validating resolver
//! from its environment: the production twins always return `false`.
//! `test-util` is not a declared feature of `webfang_core`, so `cfg(test)` is
//! the sole confinement; adding the feature would require a manifest change.

/// Test-build read of the redirect-guard hatch.
#[cfg(test)]
#[must_use]
pub(crate) fn redirect_guard_disabled_by_env() -> bool {
    std::env::var(super::DISABLE_REDIRECT_GUARD_ENV).is_ok()
}

/// Production twin: the redirect guard is always armed (S9).
#[cfg(not(test))]
#[must_use]
pub(crate) fn redirect_guard_disabled_by_env() -> bool {
    false
}

/// Test-build read of the validating-resolver hatch; exact `"1"` disarms.
#[cfg(test)]
#[must_use]
pub(crate) fn validating_resolver_disabled_by_env() -> bool {
    std::env::var(super::DISABLE_VALIDATING_RESOLVER_ENV).as_deref() == Ok("1")
}

/// Production twin: the validating resolver is always armed (S9).
#[cfg(not(test))]
#[must_use]
pub(crate) fn validating_resolver_disabled_by_env() -> bool {
    false
}
