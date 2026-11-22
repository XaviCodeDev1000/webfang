//! Slice-3 pins for the surface DEFERRED from the OptionsSpec (ADR-002):
//! hand-built args the migrated command must keep reproducing byte-for-byte,
//! the cfg-combination identity of the feature-gated flags, and the error
//! contract for out-of-bounds and malformed inputs. Split from
//! `spec_parity_tests` (spec-covered parity) at the deferred-from-spec seam.
use crate::cli::args::test_support::parse_args_hermetic;

use super::spec_parity_tests::{arg_by_id, command_args};

/// Slice 3 pin: exact surface of every field DEFERRED from the spec,
/// which the migrated command must keep building by hand. Help texts
/// are byte-exact transcriptions (final period stripped by clap's doc
/// rendering on SHORT help; multi-paragraph docs split into help +
/// long_help). Split per arg to stay under the #516 ratchets.
#[test]
fn deferred_concurrency_surface_is_pinned_for_spec_build() {
    let args = command_args();
    let c = arg_by_id(&args, "concurrency");
    assert_eq!(c.get_long(), Some("concurrency"));
    assert_eq!(c.get_short(), None);
    assert_eq!(
        c.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_CONCURRENCY")
    );
    assert!(matches!(c.get_action(), clap::ArgAction::Set));
    assert_eq!(
        c.get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["auto"]
    );
    assert_eq!(help_of(c), "Concurrency level (auto or number, minimum 1)");
    assert!(c.get_long_help().is_none());
}

#[test]
fn deferred_rate_limit_burst_surface_is_pinned_for_spec_build() {
    let args = command_args();
    let r = arg_by_id(&args, "rate_limit_burst");
    assert_eq!(r.get_long(), Some("rate-limit-burst"));
    assert_eq!(
        r.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_RATE_LIMIT_BURST")
    );
    assert!(matches!(r.get_action(), clap::ArgAction::Set));
    assert!(r.get_default_values().is_empty());
    assert_eq!(
        r.get_help()
            .expect("rate-limit-burst must carry short help")
            .to_string()
            .trim(),
        "Explicit rate-limiter burst permits (token-bucket capacity)"
    );
    let long = r
        .get_long_help()
        .expect("rate-limit-burst must carry long help")
        .to_string();
    assert!(long.contains("Overrides the hardware-derived budget-model default"));
    // The help must advertise the keyword the parser accepts, and the
    // fail-closed contract that replaced the old warn-and-default arm
    // (#1813). Before this it still claimed a "warn-and-default semantic"
    // in both directions, which no longer exists.
    assert!(
        long.contains("the keyword `auto`"),
        "long help must document the accepted `auto` keyword, got: {long}"
    );
    assert!(
        long.contains("FAILS CLOSED"),
        "long help must state the fail-closed contract, got: {long}"
    );
    // Empirical byte truth: the LONG form keeps the final period even
    // though the SHORT form strips it.
    assert!(long
        .trim_end()
        .ends_with(r#"only `auto` and an empty value mean "not set"."#));
}

#[test]
fn deferred_pattern_surfaces_are_pinned_for_spec_build() {
    let args = command_args();

    // --include-pattern <INCLUDE_PATTERNS>
    let inc = arg_by_id(&args, "include_patterns");
    assert_eq!(inc.get_long(), Some("include-pattern"));
    assert_eq!(
        inc.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_INCLUDE")
    );
    assert!(matches!(inc.get_action(), clap::ArgAction::Append));
    assert_eq!(inc.get_value_delimiter(), Some(','));
    assert_eq!(
        inc.get_help()
            .expect("include-pattern must carry short help")
            .to_string()
            .trim(),
        "URL patterns to include (glob-style). Three modes:"
    );
    assert!(inc.get_long_help().is_some());

    // --exclude-pattern <EXCLUDE_PATTERNS>
    let exc = arg_by_id(&args, "exclude_patterns");
    assert_eq!(exc.get_long(), Some("exclude-pattern"));
    assert_eq!(
        exc.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_EXCLUDE")
    );
    assert!(matches!(exc.get_action(), clap::ArgAction::Append));
    assert_eq!(exc.get_value_delimiter(), Some(','));
    assert_eq!(
        help_of(exc),
        "URL patterns to exclude (glob-style, same three modes as --include-pattern). Deny takes precedence over allow"
    );
    assert!(exc.get_long_help().is_none());
}

#[test]
fn deferred_header_and_cookie_surfaces_are_pinned_for_spec_build() {
    let args = command_args();

    // -H, --header <NAME: VALUE>
    let h = arg_by_id(&args, "headers");
    assert_eq!(h.get_long(), Some("header"));
    assert_eq!(h.get_short(), Some('H'));
    assert_eq!(
        h.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_HEADER")
    );
    assert!(matches!(h.get_action(), clap::ArgAction::Append));
    assert_eq!(h.get_value_delimiter(), Some(';'));
    let names: Vec<String> = h
        .get_value_names()
        .unwrap_or_default()
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert_eq!(names, vec!["NAME: VALUE"]);
    assert!(h.get_long_help().is_some());

    // --cookie <NAME=VALUE>
    let ck = arg_by_id(&args, "cookies");
    assert_eq!(ck.get_long(), Some("cookie"));
    assert_eq!(ck.get_short(), None);
    assert_eq!(
        ck.get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_COOKIE")
    );
    assert!(matches!(ck.get_action(), clap::ArgAction::Append));
    assert_eq!(ck.get_value_delimiter(), Some(';'));
    let names: Vec<String> = ck
        .get_value_names()
        .unwrap_or_default()
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert_eq!(names, vec!["NAME=VALUE"]);
    assert!(ck.get_long_help().is_some());
}

/// Slice 3 pin: the flags keep their identity in BOTH compile
/// configurations.
///
/// The two halves of this test now assert DIFFERENT contracts, and the
/// split is deliberate:
///
/// - `clean_ai` is UNGATED (ADR-0004 slice C), so it is asserted
///   UNCONDITIONALLY — same help, same visibility, same visible alias in
///   every configuration. That is what the test's name asks for
///   ("identity ACROSS cfg combinations"), so it is asserted rather than
///   branched.
/// - `adaptive_selectors` is still genuinely gated, so its `cfg` split
///   remains the correct assertion and is untouched.
#[test]
fn feature_gated_flags_keep_identity_across_cfg_combinations() {
    let args = command_args();

    let clean = arg_by_id(&args, "clean_ai");
    assert_eq!(clean.get_long(), Some("clean-ai"));
    assert_eq!(
        clean
            .get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_CLEAN_AI")
    );
    assert!(matches!(clean.get_action(), clap::ArgAction::SetTrue));
    assert_eq!(
        clean
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["false"]
    );
    // `clean_ai` asserts the NEW contract, with no `cfg` at all: the
    // semantic cleaner no longer needs ONNX to exist, so the flag is
    // offered in EVERY build.
    //
    // Visible with its REAL help is the correct outcome, and worth being
    // explicit about because "real help on a hidden flag" is incoherent:
    //
    // - The old shape hid the flag and swapped in a placeholder string.
    //   That made the flag invisible in `--help`, so an operator on a
    //   non-AI build had no way to discover it existed, and only learned
    //   of it by tripping exit 78 at runtime.
    // - Un-hiding it is what lets the same operator read what the flag
    //   does BEFORE running anything.
    // - Post-ADR-0004 paso 6 there is no `ai` cargo feature to reject by
    //   name: a requested `--clean-ai` whose wiring could not build a
    //   cleaner surfaces at run time as the export flow's Spanish
    //   ConfigError naming `--embedding-provider`.
    //
    // Keeping the help text identical in both builds is the point of the
    // assertion: the flag means the same thing whichever build renders it.
    assert_eq!(
        help_of(clean),
        "Use AI-powered semantic cleaning for better RAG output",
        "clean_ai must carry its real help in EVERY configuration; a \
         placeholder here is the pre-slice-C contract"
    );
    assert!(
        !clean.is_hide_set(),
        "clean_ai must stay VISIBLE in every configuration: hiding it \
         reintroduces the undiscoverable flag the slice removed"
    );
    let vis: Vec<&str> = clean.get_visible_aliases().into_iter().flatten().collect();
    assert_eq!(
        vis,
        vec!["ai"],
        "visible alias `ai` must be offered in every configuration"
    );

    let adaptive = arg_by_id(&args, "adaptive_selectors");
    assert_eq!(adaptive.get_long(), Some("adaptive-selectors"));
    assert_eq!(
        adaptive
            .get_env()
            .map(|e| e.to_string_lossy().into_owned())
            .as_deref(),
        Some("WEBFANG_ADAPTIVE_SELECTORS")
    );
    assert!(matches!(adaptive.get_action(), clap::ArgAction::SetTrue));
    assert_eq!(
        adaptive
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["false"]
    );
    if cfg!(feature = "adaptive-selectors") {
        assert_eq!(
            help_of(adaptive),
            "Enable adaptive CSS selector repair (2-tier cascade)"
        );
    } else {
        assert_eq!(
            help_of(adaptive),
            "Feature flag placeholder when adaptive-selectors is not enabled"
        );
    }
    assert_eq!(
        cfg!(feature = "adaptive-selectors"),
        !adaptive.is_hide_set()
    );
}

fn help_of(arg: &clap::Arg) -> String {
    arg.get_long_help()
        .or_else(|| arg.get_help())
        .unwrap_or_else(|| panic!("arg `{}` has no help text", arg.get_id()))
        .to_string()
        .trim()
        .to_owned()
}

#[test]
fn out_of_bounds_and_malformed_inputs_error_exactly_as_before() {
    let err = parse_args_hermetic(&["--max-pages", "0"]).expect_err("zero pages rejected");
    assert!(
        err.contains("--max-pages debe ser >= 1 (0 no deja páginas para scrapear)"),
        "got: {err}"
    );

    let err = parse_args_hermetic(&["--max-pages", "abc"]).expect_err("text rejected");
    assert!(
        err.contains("'abc' no es un número válido para --max-pages"),
        "got: {err}"
    );

    let err = parse_args_hermetic(&["--timeout-secs", "0"]).expect_err("zero timeout rejected");
    assert!(
        err.contains("--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)"),
        "got: {err}"
    );

    let err =
        parse_args_hermetic(&["--timeout-secs", "9x"]).expect_err("malformed timeout rejected");
    assert!(
        err.contains("'9x' no es un número válido para --timeout-secs"),
        "got: {err}"
    );

    let err = parse_args_hermetic(&["--download-concurrency", "0"])
        .expect_err("zero concurrency rejected");
    assert!(
        err.contains("--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)"),
        "got: {err}"
    );

    let err = parse_args_hermetic(&["--download-concurrency", "muchas"])
        .expect_err("malformed concurrency rejected");
    assert!(
        err.contains("'muchas' no es un número válido para --download-concurrency"),
        "got: {err}"
    );
}

/// P4-2 of #1296: `--download-timeout` honors the Zero Silent Loss
/// policy — 0 is rejected with the same rationale shape as
/// `--timeout-secs` (0-second timeout fails every asset instantly).
#[test]
fn download_timeout_zero_and_malformed_error_like_timeout_secs() {
    let err = parse_args_hermetic(&["--download-timeout", "0"])
        .expect_err("zero download timeout rejected");
    assert!(
        err.contains(
            "--download-timeout debe ser >= 1 (0 hace que cada descarga de assets falle al instante)"
        ),
        "got: {err}"
    );

    let err = parse_args_hermetic(&["--download-timeout", "fast"])
        .expect_err("malformed download timeout rejected");
    assert!(
        err.contains("'fast' no es un número válido para --download-timeout"),
        "got: {err}"
    );
}

/// P4-2 of #1296: `--concurrency` honors the Zero Silent Loss policy —
/// the old silent clamp 0 -> 1 is now a Spanish usage error at the argv
/// boundary (flag and `WEBFANG_CONCURRENCY` share the parser).
#[test]
fn parse_concurrency_rejects_zero_with_spanish_error() {
    let err = parse_args_hermetic(&["--concurrency", "0"]).expect_err("zero concurrency rejected");
    assert!(
        err.contains("--concurrency debe ser >= 1 (usa 'auto' para autodetectar)"),
        "got: {err}"
    );
}
