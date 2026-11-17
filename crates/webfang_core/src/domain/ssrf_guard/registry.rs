//! Process-wide SSRF guard registry — the [`ssrf_guard`] accessor and the
//! [`set_ssrf_guard`] arming entry point, extracted verbatim from the
//! `ssrf_guard` parent (ADR-0021 Slice 1 needed headroom under the 500-line
//! file ceiling; pure move, no behavior change).

use std::sync::{Arc, OnceLock};

use super::{DefaultSsrfGuard, SsrfGuard};

/// Process-wide SSRF guard instance, populated by the composition root
/// (`Container::new`) at startup and read by call sites that build HTTP
/// clients but cannot reach into the infrastructure layer.
///
/// The static is typed as the **domain** trait [`SsrfGuard`] so the domain
/// layer never names the infrastructure concrete — the trait impl for
/// [`DefaultSsrfGuard`] lives in the infrastructure `ssrf` module, reachable
/// only through this trait object.
static SSRF_GUARD: OnceLock<Arc<dyn SsrfGuard>> = OnceLock::new();

/// Install the process-wide SSRF guard. Idempotent: a second call is a
/// no-op when the first value is already set (keep-first, #996). Called by
/// the composition root (`Container::new`) at startup.
///
/// Arming-order caveat: the arming only wins the race if no client was built
/// earlier in the process — earlier builds materialize the fallback through
/// [`ssrf_guard`]. This cannot change behavior: the fallback and the armed
/// guard are behaviorally identical (`DefaultSsrfGuard` both ways), so arming
/// order only matters for test sentinels and future overrides.
pub fn set_ssrf_guard(guard: Arc<dyn SsrfGuard>) {
    let _ = SSRF_GUARD.set(guard);
}

/// Test-only probe: was the registry armed by a composition root?
///
/// The armed guard and the fallback are behaviorally identical
/// (`DefaultSsrfGuard` both ways), so [`ssrf_guard`] cannot distinguish
/// them; wiring tests need this probe to prove `Container::new` actually
/// calls [`set_ssrf_guard`] (strict-TDD RED without the arm).
#[cfg(test)]
pub(crate) fn ssrf_guard_armed() -> bool {
    SSRF_GUARD.get().is_some()
}

/// Read the process-wide SSRF guard. When no composition root has armed one,
/// falls back to the self-sufficient [`DefaultSsrfGuard`] (whose impl applies
/// the full guard — redirect policy + validating resolver) so paths that build
/// HTTP clients while bypassing `Container` (CLI preflight, crawler discovery,
/// sitemap discovery) are protected exactly like Container-wired clients.
///
/// The fallback is materialized once, at first use, and never replaced by a
/// later arming (the registry is keep-first).
#[must_use]
pub fn ssrf_guard() -> Arc<dyn SsrfGuard> {
    if let Some(guard) = SSRF_GUARD.get() {
        return guard.clone();
    }
    static FALLBACK: OnceLock<Arc<dyn SsrfGuard>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| Arc::new(DefaultSsrfGuard) as Arc<dyn SsrfGuard>)
        .clone()
}
