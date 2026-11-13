use super::*;

// ========================================================================
// #1813 — --adaptive-selectors preflight feature check
// ========================================================================

/// Build opts with `--adaptive-selectors` requested.
fn adaptive_selectors_opts() -> CrawlOptions {
    CrawlOptions {
        adaptive_selectors: true,
        ..CrawlOptions::default()
    }
}

/// `--adaptive-selectors` without the feature: config error naming the
/// feature, before any network request (#1813).
#[test]
fn adaptive_selectors_without_feature_errors() {
    let err = check_adaptive_selectors_feature_with(false, &adaptive_selectors_opts())
        .expect_err("non-adaptive build must reject --adaptive-selectors in preflight");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
            "config error must name the flag and the feature, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// `--adaptive-selectors` with the feature compiled in: passes.
#[test]
fn adaptive_selectors_with_feature_ok() {
    assert!(check_adaptive_selectors_feature_with(true, &adaptive_selectors_opts()).is_ok());
}

/// No `--adaptive-selectors`: passes regardless of the feature state, so
/// the gate never blocks a run that did not ask for the capability.
#[test]
fn no_adaptive_selectors_ok_without_feature() {
    assert!(check_adaptive_selectors_feature_with(false, &CrawlOptions::default()).is_ok());
}

// ---- #1813 S1: the gate is reached from the SHARED staging pipeline ----

/// `Args` that request the capability, i.e. what argv or
/// `WEBFANG_ADAPTIVE_SELECTORS` produces before `normalize` runs.
fn adaptive_selectors_args() -> Args {
    let mut args = Args::default();
    args.crawler.adaptive_selectors = true;
    args
}

/// The regression S1 is about, in the lane CI actually runs.
///
/// The gate used to be reachable ONLY from `webfang_cli/src/main.rs` step
/// 6e2, so this `Args`-shaped seam did not exist at all. Now the same
/// request, read off `Args` exactly the way `normalize` reads it, is
/// rejected with exit 78 and the feature-naming message.
///
/// This runs in EVERY lane including `--all-features`, which is the whole
/// point of the injection seam: without it, a gate wired this way would be
/// invisible to CI for the same reason the original defect was.
#[test]
fn adaptive_capability_gate_from_args_rejects_request_without_the_feature() {
    let err = check_adaptive_selectors_capability_with(false, &adaptive_selectors_args())
        .expect_err("a non-adaptive build must reject the request during normalization");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
            "config error must name the flag and the feature, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// With the feature: the same `Args` passes, so the gate never blocks a
/// run on a build that CAN honor the request.
#[test]
fn adaptive_capability_gate_from_args_accepts_request_with_the_feature() {
    assert!(check_adaptive_selectors_capability_with(true, &adaptive_selectors_args()).is_ok());
}

/// No request: passes regardless of the feature state.
#[test]
fn adaptive_capability_gate_from_args_is_silent_without_the_request() {
    assert!(check_adaptive_selectors_capability_with(false, &Args::default()).is_ok());
}

/// The two input shapes must not drift into two different decisions — they
/// share one semantic, and a run reaching `normalize` is subject to exactly
/// the same verdict a programmatic `CrawlOptions` builder would get.
#[test]
fn adaptive_capability_gate_agrees_across_both_input_shapes() {
    for adaptive_enabled in [false, true] {
        let from_args =
            check_adaptive_selectors_capability_with(adaptive_enabled, &adaptive_selectors_args());
        let from_opts =
            check_adaptive_selectors_feature_with(adaptive_enabled, &adaptive_selectors_opts());
        assert_eq!(
            from_args.is_ok(),
            from_opts.is_ok(),
            "the Args-shaped and CrawlOptions-shaped gates must reach the same \
             verdict (adaptive_enabled={adaptive_enabled})"
        );
    }
}
