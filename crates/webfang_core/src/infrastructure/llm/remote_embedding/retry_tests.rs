use super::*;

// --- Backoff policy (no sleeps) ---

#[test]
fn backoff_honors_max_of_retry_after_and_exponential() {
    // Retry-After: 3s → 3000ms beats exponential attempt 2 (2000ms).
    assert_eq!(backoff_delay_ms(2, Some(Duration::from_secs(3))), 3000);
    // Retry-After: 1s → 1000ms loses to exponential attempt 3 (4000ms).
    assert_eq!(backoff_delay_ms(3, Some(Duration::from_secs(1))), 4000);
    // Absent/zero → pure exponential from the 1000ms base.
    assert_eq!(backoff_delay_ms(1, None), 1000);
    assert_eq!(backoff_delay_ms(2, None), 2000);
    assert_eq!(backoff_delay_ms(2, Some(Duration::ZERO)), 2000);
}

// --- Retry-After wire forms (RFC 9110 §10.2.3) ---
//
// `now` is injected, so these are exact value assertions rather than
// "roughly a minute" timing guesses.

/// The single instant every `Retry-After` test measures against:
/// `1994-11-06T08:49:37Z`, RFC 9110's own example date — so the header
/// values below are readably "60s away".
fn utc_fixture() -> DateTime<Utc> {
    DateTime::parse_from_rfc2822("Sun, 06 Nov 1994 08:49:37 GMT")
        .expect("RFC 2822 fixture parses")
        .with_timezone(&Utc)
}

#[test]
fn retry_after_delay_seconds_form_is_honored() {
    let now = utc_fixture();
    // "120" parses as a plain 120s instruction, but it is above the 60s
    // ceiling, so it is honored *bounded*: this row now pins the clamp,
    // not a silent cap (see the hostile-header test below for the
    // under-ceiling counterpart that must stay untouched).
    assert_eq!(
        retry_after_delay(Some("120"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
        "a 120s delay-seconds is parsed, then clamped to the honored ceiling"
    );
    // Surrounding whitespace is legal header framing.
    assert_eq!(
        retry_after_delay(Some(" 7 "), now),
        Some(Duration::from_secs(7))
    );
}

/// A `delay-seconds` header is an unbounded `u64`, and
/// `Duration::from_secs` accepts all of it — so `u64::MAX` used to become
/// a `Duration` whose millisecond conversion saturates, and the adapter
/// slept for ~584M years instead of failing or retrying (#1956,
/// `R3-overflow-saturation`). Pure, so this asserts in microseconds.
#[test]
fn retry_after_hostile_delay_seconds_is_clamped_not_slept() {
    let now = utc_fixture();
    assert_eq!(
        retry_after_delay(Some("18446744073709551615"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
        "u64::MAX seconds must clamp to the honored ceiling, never become a sleep"
    );
    // End to end through the sleep computation: the value handed to
    // `tokio::time::sleep` is bounded even though nothing sleeps here.
    assert_eq!(
        backoff_delay_ms(1, retry_after_delay(Some("18446744073709551615"), now)),
        RETRY_AFTER_MAX_MS,
        "the ms handed to tokio::time::sleep must be bounded by the ceiling"
    );
    // Just above the ceiling behaves the same way.
    assert_eq!(
        retry_after_delay(Some("61"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
        "61s is one second past the ceiling and must be clamped"
    );
}

/// The same class of hostile input through the other wire form: an
/// HTTP-date far enough out that its delta does not even fit in a
/// `Duration` (~7973 years), which the old `to_std().unwrap_or(ZERO)`
/// silently answered as "retry immediately".
#[test]
fn retry_after_far_future_http_date_is_clamped_to_the_ceiling() {
    let now = utc_fixture();
    assert_eq!(
        retry_after_delay(Some("Fri, 31 Dec 9999 23:59:59 GMT"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
        "a year-9999 date must clamp to the ceiling, not saturate or sleep"
    );
    // A date inside the representable range but past the ceiling takes
    // the same path, so the bound does not depend on the overflow edge.
    assert_eq!(
        retry_after_delay(Some("Sun, 06 Nov 1994 08:51:37 GMT"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
        "a 120s HTTP-date is over the ceiling and must clamp"
    );
}

/// The counterweight to the two tests above: the ceiling must bound
/// hostile input **without** quietly capping normal operation. Every value
/// here is honored exactly, to the millisecond.
#[test]
fn retry_after_under_the_ceiling_is_honored_exactly() {
    let now = utc_fixture();
    assert_eq!(
        retry_after_delay(Some("30"), now),
        Some(Duration::from_secs(30)),
        "a legitimate 30s delay-seconds must not be clamped"
    );
    assert_eq!(
        retry_after_delay(Some("Sun, 06 Nov 1994 08:50:07 GMT"), now),
        Some(Duration::from_secs(30)),
        "a legitimate 30s HTTP-date must not be clamped"
    );
    // The boundary is inclusive: exactly the ceiling is honored, so the
    // clamp can never shave a millisecond off a compliant server.
    assert_eq!(
        retry_after_delay(Some("60"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS))
    );
    assert_eq!(
        retry_after_delay(Some("Sun, 06 Nov 1994 08:50:37 GMT"), now),
        Some(Duration::from_millis(RETRY_AFTER_MAX_MS))
    );
    // And it still flows into the sleep as asked, beating the exponential
    // floor (1000ms) exactly as a 30s instruction should.
    assert_eq!(
        backoff_delay_ms(1, retry_after_delay(Some("30"), now)),
        30_000
    );
}

#[test]
fn retry_after_http_date_form_is_honored() {
    // The bug this pins: the IMF-fixdate form used to fail a
    // `.parse::<u64>()`, be discarded silently, and downgrade a
    // server's explicit wait to pure exponential backoff.
    let now = utc_fixture();
    assert_eq!(
        retry_after_delay(Some("Sun, 06 Nov 1994 08:50:37 GMT"), now),
        Some(Duration::from_secs(60)),
        "IMF-fixdate must convert to a 60s delay against the injected now"
    );
    // Measured limitation (chrono 0.4.45): neither obsolete HTTP-date form
    // parses, so each degrades to exponential backoff behind the
    // caller's warn!. Pinned so a chrono upgrade that starts accepting
    // them shows up as a deliberate change, not silent drift.
    assert_eq!(
        retry_after_delay(Some("Sunday, 06-Nov-94 08:50:37 GMT"), now),
        None,
        "RFC 850 is not parsed by chrono 0.4.45; it must degrade, not panic"
    );
    assert_eq!(
        retry_after_delay(Some("Sun Nov  6 08:50:37 1994"), now),
        None,
        "asctime is not parsed by chrono 0.4.45; it must degrade, not panic"
    );
}

#[test]
fn retry_after_past_date_clamps_to_zero_never_negative() {
    let now = utc_fixture();
    let past = "Sun, 06 Nov 1994 08:49:37 GMT";
    assert_eq!(
        retry_after_delay(Some(past), now),
        Some(Duration::ZERO),
        "a stale date means retry-now and must not underflow"
    );
    // Exactly-now is also "not in the future".
    assert_eq!(
        retry_after_delay(Some(past), now),
        Some(Duration::ZERO),
        "a date equal to now clamps the same way"
    );
    // And the zero delay leaves the exponential floor standing alone.
    assert_eq!(
        backoff_delay_ms(1, retry_after_delay(Some(past), now)),
        1000
    );
}

#[test]
fn retry_after_absent_empty_or_garbage_is_none() {
    let now = utc_fixture();
    assert_eq!(retry_after_delay(None, now), None, "absent header");
    assert_eq!(retry_after_delay(Some(""), now), None, "empty header");
    assert_eq!(retry_after_delay(Some("   "), now), None, "blank header");
    assert_eq!(retry_after_delay(Some("pronto"), now), None, "garbage");
    assert_eq!(
        retry_after_delay(Some("-5"), now),
        None,
        "negative delay-seconds is not a valid Retry-After"
    );
    assert_eq!(
        retry_after_delay(Some("Sun, 99 Xxx 1994 08:49:37 GMT"), now),
        None,
        "an unparsable date falls back rather than panicking"
    );
    // None means "exponential only", never "sleep zero".
    assert_eq!(
        backoff_delay_ms(2, retry_after_delay(Some("pronto"), now)),
        2000
    );
}
