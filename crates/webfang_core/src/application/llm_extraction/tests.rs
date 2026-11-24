use super::*;

#[test]
fn ssrf_gate_blocks_forbidden_internal_ips() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[LLM_SSRF_TEST_HATCH_ENV]);
    for host in [
        "127.0.0.1",
        "169.254.169.254",
        "10.0.0.1",
        "100.64.1.1",
        "[fc00::1]",
    ] {
        let url = Url::parse(&format!("http://{host}/v1")).expect("literal parses");
        let err = ssrf_gate(&url).expect_err("forbidden host must be blocked");
        assert!(
            matches!(err, ScraperError::Config(_)),
            "host {host}: {err:?}"
        );
    }
}

#[test]
fn ssrf_gate_allows_public_ip() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[LLM_SSRF_TEST_HATCH_ENV]);
    let url = Url::parse("https://8.8.8.8/v1").expect("literal parses");
    ssrf_gate(&url).expect("public host must pass");
}

#[test]
fn ssrf_gate_rejects_non_http_scheme() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[LLM_SSRF_TEST_HATCH_ENV]);
    let url = Url::parse("ftp://8.8.8.8/v1").expect("parses");
    assert!(matches!(
        ssrf_gate(&url).expect_err("ftp must fail"),
        ScraperError::Config(_)
    ));
}

/// #1615 DF-E9: the hatch still disarms the gate for THIS crate's unit
/// tests — the change is that it is no longer compiled into production,
/// not that the tests lost their loopback escape. A value that looks like
/// a refusal (`0`, `false`, empty) still disarms it, because that is the
/// pre-existing unit-test contract and hardening it would change behaviour
/// only under `cfg(test)`, which is not where the finding lived.
///
/// The STRONG row is `crates/webfang_core/tests/llm_ssrf_hatch_test.rs`,
/// which is compiled as an EXTERNAL crate where `cfg(test)` of
/// `webfang_core` is definitively off. A unit test cannot observe the
/// property that matters here, because a unit test is compiled WITH it —
/// so this test pins that the escape still works, and that other one pins
/// that production cannot reach it.
#[test]
fn ssrf_gate_bypass_env_for_tests() {
    let _guard = webfang_test_utils::EnvGuard::with(&[(LLM_SSRF_TEST_HATCH_ENV, "1")]);
    let url = Url::parse("http://127.0.0.1:9/v1").expect("parses");
    assert!(ssrf_gate(&url).is_ok());
}

#[test]
fn from_env_missing_key_is_config_error() {
    let _guard = webfang_test_utils::EnvGuard::clean(&["LLM_API_KEY"]);
    let err = LlmConfig::from_env("https://8.8.8.8/v1", 100, 800).expect_err("missing key fails");
    assert!(matches!(err, ScraperError::Config(_)));
}

#[test]
fn from_env_redacts_key_in_debug() {
    let _guard = webfang_test_utils::EnvGuard::with(&[("LLM_API_KEY", "sk-super-secret")]);
    let cfg = LlmConfig::from_env("https://8.8.8.8/v1", 100, 800).expect("key present");
    let dbg = format!("{cfg:?}");
    assert!(dbg.contains("[REDACTED]"), "debug must redact: {dbg}");
    assert!(
        !dbg.contains("sk-super-secret"),
        "debug leaked secret: {dbg}"
    );
}

/// S10/T2b: the completion guard takes the SAME resolved number the
/// chunker and the diagnostics use. `max_tokens` stays output-only — it
/// never re-derives a character budget from a tokens→chars ratio.
#[test]
fn completion_char_budget_is_the_governing_resolution() {
    let _guard = webfang_test_utils::EnvGuard::with(&[("LLM_API_KEY", "sk-test")]);
    use crate::domain::model_profile::{
        governed_max_chars, ModelProfileOverride, ModelProfilesConfig, GRANITE_97M_PROFILE_ID,
    };
    let profiles = ModelProfilesConfig {
        profiles: vec![ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            context: Some(1024),
            chars_per_token: Some(2.0),
            ..ModelProfileOverride::default()
        }],
    };
    let governed = governed_max_chars(None, &profiles).expect("resolves");
    assert_eq!(governed, 2048, "the profile governs the budget");
    let cfg = LlmConfig::from_env("https://8.8.8.8/v1", 100, governed).expect("key present");
    assert_eq!(
        cfg.char_budget, governed,
        "the completion guard uses it verbatim"
    );
    assert_eq!(cfg.max_tokens, 100, "max_tokens remains the OUTPUT budget");
}
