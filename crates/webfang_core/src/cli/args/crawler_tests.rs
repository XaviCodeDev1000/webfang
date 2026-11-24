use super::*;
use crate::domain::config::ConcurrencyConfig;

#[test]
fn parse_timeout_secs_accepts_valid_value() {
    assert_eq!(parse_timeout_secs("30"), Ok(30));
}

#[test]
fn parse_timeout_secs_rejects_zero() {
    let err = parse_timeout_secs("0").unwrap_err();
    assert_eq!(
        err,
        "--timeout-secs must be >= 1 (0 makes every request fail instantly)"
    );
}

#[test]
fn parse_timeout_secs_rejects_non_numeric() {
    let err = parse_timeout_secs("abc").unwrap_err();
    assert_eq!(err, "'abc' is not a valid number for --timeout-secs");
}

#[test]
fn parse_rate_limit_burst_accepts_positive_values() {
    assert_eq!(parse_rate_limit_burst("1"), Ok(Some(1)));
    assert_eq!(parse_rate_limit_burst("5"), Ok(Some(5)));
    assert_eq!(parse_rate_limit_burst("64"), Ok(Some(64)));
}

#[test]
fn parse_rate_limit_burst_rejects_zero_with_spanish_error() {
    // Same "Zero Silent Loss" boundary as --download-concurrency (D1).
    let err = parse_rate_limit_burst("0").unwrap_err();
    assert_eq!(
        err,
        "--rate-limit-burst must be >= 1 (0 allows no requests in a burst)"
    );
}

#[test]
fn parse_rate_limit_burst_non_numeric_fails_closed_with_spanish_error() {
    // #1813: the last fail-open arm. A typo'd value used to degrade to the
    // derived default in silence (only a pre-logging note), so the operator
    // got a different request cadence than the one they asked for.
    //
    // `auto` is DELIBERATELY ABSENT from this list: it is not a typo, it is
    // a keyword the sibling `--concurrency` parser understands, and
    // `WEBFANG_RATE_LIMIT_BURST=auto` was a WORKING spelling before #1813.
    // See `parse_rate_limit_burst_accepts_sibling_auto_keyword`.
    for raw in ["abc", "banana", "1_0", "12x"] {
        let err = parse_rate_limit_burst(raw)
            .err()
            .unwrap_or_else(|| panic!("{raw} must be rejected, not defaulted"));
        assert!(
            err.starts_with("--rate-limit-burst") && err.contains("is not a number"),
            "expected a Spanish not-a-number error for {raw}, got: {err}"
        );
    }
}

/// #1813 regression fix: the fail-closed arm must turn away TYPOS, not the
/// keywords the sibling `--concurrency` parser understands. `auto` means
/// "use the derived default" there, so hard-`Err`-ing on it here made every
/// deployment that pinned `WEBFANG_RATE_LIMIT_BURST=auto` abort with exit
/// 78 on upgrade — a fail-CLOSED regression, which is worse than the
/// fail-open it fixed, because it is not a silent wrong answer.
///
/// Matching is case-insensitive and whitespace-tolerant for the same reason
/// the sibling matches them: `ConcurrencyConfig::from_str` trims and
/// lowercases before comparing (`domain/config.rs:391-393`), so `AUTO` and
/// `"  auto  "` are the same request there and must be here too.
#[test]
fn parse_rate_limit_burst_accepts_sibling_auto_keyword() {
    for raw in ["auto", "AUTO", "Auto", "  auto  "] {
        assert_eq!(
            parse_rate_limit_burst(raw),
            Ok(None),
            "`{raw}` is a keyword --concurrency understands; it must mean \
             \"not set\" here, never exit 78"
        );
    }
}

/// Pins the two keyword sets together so they cannot drift apart silently.
///
/// The burst parser's accepted-keyword list is a local constant, not a read
/// of `ConcurrencyConfig` — the sibling exposes no predicate, and widening
/// its API is outside this surface. This test is what substitutes for that
/// link, and it checks BOTH directions per keyword:
///
/// 1. the sibling `--concurrency` parser understands it as auto-detect, and
/// 2. the burst parser turns it into "not set" rather than an error.
///
/// A keyword added to one side and not the other fails here, in CI, instead of
/// in a deployment. An emptied list also fails here rather than silently
/// restoring the regression, because step 2 has nothing to iterate — which is
/// why the guard does not need a separate non-empty assertion.
#[test]
fn burst_auto_keywords_match_the_concurrency_parser() {
    for kw in RATE_LIMIT_BURST_AUTO_KEYWORDS {
        let sibling: Result<ConcurrencyConfig, _> = kw.parse();
        assert!(
            sibling.is_ok_and(|c| c.is_auto()),
            "burst accepts `{kw}` as \"use the derived default\", but the sibling \
         --concurrency parser does not treat it as auto — the two keyword sets \
         have drifted apart"
        );
        assert_eq!(
            parse_rate_limit_burst(kw),
            Ok(None),
            "burst accepts `{kw}` as a keyword, so it must stage as \"not set\""
        );
    }
}

/// The one deliberate carve-out: an EMPTY value still means "not set",
/// not "invalid". `parse_rate_limit_burst` is only ever called on a
/// `Some(raw)` the operator actually supplied, and `WEBFANG_RATE_LIMIT_BURST=""`
/// is a common way to neutralise a variable — hard-failing that would break
/// scripts for no safety gain.
#[test]
fn parse_rate_limit_burst_treats_empty_as_unset() {
    assert_eq!(parse_rate_limit_burst(""), Ok(None));
    assert_eq!(parse_rate_limit_burst("   "), Ok(None));
}

/// #P4-4 item 6: an integer too wide for `u32` is an explicit request that
/// cannot be honoured, so it must fail — never silently become the derived
/// default. `u32::MAX + 1` is the exact value that used to warn and default.
#[test]
fn parse_rate_limit_burst_rejects_out_of_u32_range_with_spanish_error() {
    for raw in [
        "4294967296", // u32::MAX + 1
        "99999999999",
        "-1",                   // negative is a number too
        "18446744073709551616", // u64::MAX + 1, wider than any int type
    ] {
        let err = parse_rate_limit_burst(raw)
            .err()
            .unwrap_or_else(|| panic!("{raw} must be rejected, not defaulted"));
        assert!(
            err.starts_with("--rate-limit-burst") && err.contains("is outside the allowed range"),
            "expected a Spanish range error for {raw}, got: {err}"
        );
    }
}

/// The new rejection must stay exclusive: `u32::MAX` itself is still
/// accepted and honoured verbatim, so only values that genuinely cannot be
/// represented are turned away.
#[test]
fn parse_rate_limit_burst_accepts_the_whole_u32_range() {
    assert_eq!(parse_rate_limit_burst("4294967295"), Ok(Some(u32::MAX)));
    assert_eq!(parse_rate_limit_burst("+9"), Ok(Some(9)));
}

#[test]
fn looks_like_integer_separates_numbers_from_keywords() {
    assert!(looks_like_integer("4294967296"));
    assert!(looks_like_integer("-1"));
    assert!(looks_like_integer("+7"));
    assert!(looks_like_integer("0"));
    assert!(!looks_like_integer("auto"));
    assert!(!looks_like_integer("abc"));
    assert!(!looks_like_integer(""));
    assert!(!looks_like_integer("1.5"));
    assert!(!looks_like_integer("1_000"));
}

#[test]
fn rate_limit_burst_flag_parses_through_clap() {
    let args = crate::cli::args::test_support::parse_args_hermetic(&["--rate-limit-burst", "9"])
        .expect("valid flag combination");
    assert_eq!(args.crawler.rate_limit_burst.as_deref(), Some("9"));
}

#[test]
fn parse_max_pages_accepts_valid_value() {
    assert_eq!(parse_max_pages("5"), Ok(5));
}

#[test]
fn parse_max_pages_rejects_zero() {
    // Bug #780: max_pages 0 panics mpsc::channel(0) → SIGABRT.
    let err = parse_max_pages("0").unwrap_err();
    assert_eq!(
        err,
        "--max-pages must be >= 1 (0 leaves no pages to scrape)"
    );
}

#[test]
fn parse_max_pages_rejects_non_numeric() {
    let err = parse_max_pages("abc").unwrap_err();
    assert_eq!(err, "'abc' is not a valid number for --max-pages");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "servo_arc 0.4.3 Tree Borrows false positive in Selector::parse (scraper/selectors) — #810"
)]
fn parse_selector_accepts_valid_css() {
    assert_eq!(parse_selector("article p"), Ok("article p".to_string()));
    assert_eq!(parse_selector("body"), Ok("body".to_string()));
    assert_eq!(
        parse_selector(".content > h1"),
        Ok(".content > h1".to_string())
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "servo_arc 0.4.3 Tree Borrows false positive in Selector::parse (scraper/selectors) — #810"
)]
fn parse_selector_rejects_invalid_css() {
    // Regression for #797: an invalid CSS selector must be rejected at
    // parse time (clap error -> exit 64), not at scrape time (30s timeout).
    let err = parse_selector(":::invalid").unwrap_err();
    assert!(
        err.contains("invalid CSS selector"),
        "expected Spanish invalid-selector message, got: {err}"
    );
}
