use std::time::Duration;

use chrono::{DateTime, Utc};

use super::{BACKOFF_BASE_MS, BACKOFF_MAX_MS, RETRY_AFTER_MAX_MS};

/// Retry delay in milliseconds before attempt `attempt` (1-based): the max
/// of `Retry-After` and exponential backoff from the 1s base (capped at
/// 10s), mirroring the hardened HTTP client convention.
///
/// Pure so the policy is unit-testable without sleeping. A
/// [`Duration::ZERO`] `Retry-After` (past HTTP-date, or an explicit `0`)
/// carries no instruction, so the exponential floor stands alone.
///
/// Total by construction: the result is at most
/// [`RETRY_AFTER_MAX_MS`], because a `retry_after` argument is expected to be
/// pre-clamped by [`retry_after_delay`] and is clamped again here. The second
/// clamp is dead code for every current caller, and that is the point — this
/// function is the last thing standing between a hostile header and a
/// `tokio::time::sleep`, so its fallback must be bounded even if a future
/// caller forgets to bound its input. `u64::MAX` milliseconds would be ~584M
/// years of sleep, i.e. an indefinite hang that looks like a stuck process.
pub(super) fn backoff_delay_ms(attempt: u32, retry_after: Option<Duration>) -> u64 {
    let exponential = BACKOFF_BASE_MS
        .saturating_mul(2u64.pow(attempt.saturating_sub(1)))
        .min(BACKOFF_MAX_MS);
    match retry_after {
        Some(delay) if !delay.is_zero() => exponential.max(
            u64::try_from(delay.as_millis())
                .unwrap_or(u64::MAX)
                .min(RETRY_AFTER_MAX_MS),
        ),
        _ => exponential,
    }
}

/// Clamp one requested `Retry-After` delay to [`RETRY_AFTER_MAX_MS`].
///
/// The clamp is **not** silent: an oversized instruction is a server asking
/// for more than the adapter will ever grant, which an operator triaging a
/// stall needs to see. Only a request strictly above the ceiling warns — an
/// absent, empty, unparsable, `"0"` or past-date header carries no oversized
/// instruction and reaches here untouched.
fn clamp_retry_after(requested: Duration, raw: &str) -> Duration {
    let ceiling = Duration::from_millis(RETRY_AFTER_MAX_MS);
    if requested <= ceiling {
        return requested;
    }
    tracing::warn!(
        raw = %raw,
        // `as_millis` is a u128; saturate rather than truncate a hostile value.
        requested_ms = u64::try_from(requested.as_millis()).unwrap_or(u64::MAX),
        honored_ms = RETRY_AFTER_MAX_MS,
        "remote embedding Retry-After exceeds the honored ceiling; clamped"
    );
    ceiling
}

/// `Retry-After` header value → the delay the server asked for, in either
/// wire form RFC 9110 §10.2.3 allows.
///
/// Both forms are honored: `delay-seconds` (`"120"`) and `HTTP-date`
/// (`"Sun, 06 Nov 1994 08:49:37 GMT"`) — each bounded by
/// [`RETRY_AFTER_MAX_MS`], as described below.
///
/// `now` is a **parameter**, not a `Utc::now()` call inside: that is what
/// makes the HTTP-date branch assertable with exact values from the in-crate
/// test module — no clock mock, no sleeping. The conversion is pure, so it
/// does not belong on the public surface; what the crate's users observe is
/// the retry behavior, pinned through the public port.
///
/// [`None`] when the header is absent, empty, or parses in neither form;
/// the caller then falls back to exponential backoff (and says so in a
/// `warn!` — the discard is never silent).
///
/// A date at or before `now` yields [`Duration::ZERO`] ("retry now"),
/// clamped explicitly: a stale date must never become a negative
/// duration.
///
/// Any honored delay is clamped to [`RETRY_AFTER_MAX_MS`] here, not at the
/// call site, so both wire forms and the unit tests share one bound.
///
/// # Observability
///
/// The oversized case warns **from inside this function**
/// ([`clamp_retry_after`]) rather than being handed back for the caller to
/// warn about. Two reasons, both about this function's signature: it returns
/// a bare `Option<Duration>`, so a clamp is only detectable here *before* the
/// requested magnitude is discarded — afterwards `RETRY_AFTER_MAX_MS` is
/// indistinguishable from a legitimate `Retry-After: 60`, and a caller-side
/// comparison would double-warn on every honest header that happens to sit on
/// the boundary. The call site keeps its own `warn!` for the case this one
/// cannot cover (unparsable → `None`), carrying the `provider_id`; this
/// warning carries the raw value and both magnitudes, which is what identifies
/// the hostile header in a trace.
pub(super) fn retry_after_delay(header: Option<&str>, now: DateTime<Utc>) -> Option<Duration> {
    let raw = header?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(clamp_retry_after(Duration::from_secs(secs), raw));
    }
    // RFC 9110 §10.2.3 names three HTTP-date forms. Verified empirically
    // against chrono 0.4.45: `parse_from_rfc2822` accepts IMF-fixdate
    // (the one servers actually send, and the regression this unit
    // fixes); neither obsolete form — asctime nor RFC 850 — parses. An
    // unsupported form degrades to exponential backoff behind the
    // explicit `warn!` above: never silent, never a wrong sleep.
    let target = DateTime::parse_from_rfc2822(raw).ok()?;
    let target = target.with_timezone(&Utc);
    if target <= now {
        return Some(Duration::ZERO);
    }
    // A delta too large for `to_std` (well past the year-9999 case, which
    // chrono converts fine at ~2.5e11 s) is still an *oversized*
    // instruction, not a retry-now one: saturate to `Duration::MAX` and let
    // the shared clamp decide. The previous `unwrap_or(Duration::ZERO)`
    // turned such a date into an immediate retry — the opposite of what the
    // server asked for.
    let requested = (target - now).to_std().unwrap_or(Duration::MAX);
    Some(clamp_retry_after(requested, raw))
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
