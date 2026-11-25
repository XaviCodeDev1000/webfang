use super::fixtures::*;
use super::*;

#[test]
fn display_for_id_derives_a_readable_label() {
    assert_eq!(display_for_id("connect-f82b"), "Connect F82b");
    assert_eq!(display_for_id("acme-inc-99"), "Acme Inc 99");
    assert_eq!(display_for_id("connect-x"), "Connect X");
}

/// A valid `[[providers]]` block PLUS a mistyped unrelated top-level key
/// (`format = 42`) used to pass the gate: `ProvidersConfig` models one
/// field and ignores the rest, so connect printed success and wrote — and
/// the next run degraded EVERYTHING to defaults in silence. The gate must
/// validate the same document the run loads, so the whole flow (probe OK,
/// write refused) ends in `CliExit::ConfigError` with the file untouched.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn connect_refuses_a_config_with_a_mistyped_unrelated_top_level_key() {
    let server = serve_embeddings().await;
    let (_dir, path) = temp_config();
    let seeded = "\
format = 42

[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
    seed_config(&path, seeded);
    let before = std::fs::read(&path).expect("read seed");

    let provider = loopback_provider(&server, "connect-a1b2");

    // The probe answers; only the pre-write gate can refuse. The incoming
    // id differs from the seeded one on purpose: the merge /
    // cross-endpoint guards would refuse on their own and mask the gate.
    let exit = register(&provider, &path).await;
    match exit {
        CliExit::ConfigError(msg) => {
            assert!(
                msg.contains("documento completo"),
                "the refusal must name the full-document gate: {msg}"
            );
            assert!(
                msg.contains("config.toml"),
                "the refusal must name the file: {msg}"
            );
        },
        other => panic!("a mistyped config key must be a ConfigError, got {other:?}"),
    }

    assert_eq!(
        before,
        std::fs::read(&path).expect("read back"),
        "a refused write must leave the config byte-for-byte identical"
    );
}

/// A valid `[[providers]]` block PLUS an invalid `[[model_profiles]]
/// .profiles]]` override (`dim = "no-numérico"`) is the same defect one
/// table further away: the run loads `model_profiles` through
/// `ConfigDefaults`, so a value it cannot deserialize must refuse the
/// write, not pass a providers-only gate (also closes R4-3).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn connect_refuses_a_config_with_an_invalid_model_profiles_override() {
    let server = serve_embeddings().await;
    let (_dir, path) = temp_config();
    let seeded = "\
[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"

[[model_profiles.profiles]]
id = \"granite-97m\"
dim = \"no-numérico\"
";
    seed_config(&path, seeded);
    let before = std::fs::read(&path).expect("read seed");

    let provider = loopback_provider(&server, "connect-a1b2");

    // The incoming id differs from the seeded one on purpose: the merge /
    // cross-endpoint guards would refuse on their own and mask the gate.
    let exit = register(&provider, &path).await;
    assert!(
        matches!(exit, CliExit::ConfigError(_)),
        "an invalid model_profiles override must be a ConfigError, got {exit:?}"
    );

    assert_eq!(
        before,
        std::fs::read(&path).expect("read back"),
        "a refused write must leave the config byte-for-byte identical"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn connect_registers_a_provider_that_answers_the_probe() {
    let server = serve_embeddings().await;
    let (_dir, path) = temp_config();

    // The guarded production client rejects loopback, so this exercises the
    // same validation/write path with `allow_loopback` set — the operator's
    // own provider would carry `false` and dial a public host.
    let provider = loopback_provider(&server, "connect-a1b2");

    let exit = register(&provider, &path).await;
    assert!(matches!(exit, CliExit::Success), "got {exit:?}");

    let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config parses");
    assert_eq!(loaded.providers.len(), 1);
    assert_eq!(loaded.providers[0].id, "connect-a1b2");
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn connect_rejects_a_dead_endpoint_and_writes_nothing() {
    // RFC 5737 TEST-NET-1, port 1: nothing listens there.
    let (_dir, path) = temp_config();

    let exit = run(
        &url::Url::parse("http://192.0.2.1:1/").expect("url"),
        "connect-dead",
        "nomic-embed-text",
        &path,
    )
    .await;

    assert!(matches!(exit, CliExit::NetworkError(_)), "got {exit:?}");
    assert!(
        !path.exists()
            || std::fs::read_to_string(&path)
                .unwrap_or_default()
                .is_empty(),
        "no provider record may be written when the endpoint does not answer"
    );
}

#[tokio::test]
async fn connect_rejects_an_illegal_provider_id_before_touching_the_network() {
    let (_dir, path) = temp_config();

    let exit = run(
        &url::Url::parse("http://192.0.2.1:1/").expect("url"),
        "has space",
        "nomic-embed-text",
        &path,
    )
    .await;

    assert!(matches!(exit, CliExit::UsageError(_)), "got {exit:?}");
    assert!(!path.exists(), "an illegal id must not write anything");
}

// ---------------------------------------------------------------------
// R1-1: the temp file of the atomic write must not be plantable through
// a symlink (CWE-377). Old shape: `.{file}.connect-{pid}.tmp`, opened
// `write + create + truncate` — a local writer in the config directory
// pre-planted that exact name with a symlink and connect truncated the
// victim through it.
// ---------------------------------------------------------------------

/// #S14: a target the SSRF entry guard forbids leaves with 70, before the
/// adapter is built and before the `ConfigError → NetworkError` remap — a
/// forbidden address is not an endpoint that failed to answer, and 69 told
/// automation to retry what must never run.
#[test]
fn connect_refuses_a_forbidden_literal_endpoint_with_exit_70() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let mut provider = test_provider("connect-ssrf");
    provider.base_url = url::Url::parse("http://10.0.0.1/v1").expect("url");
    provider.allow_loopback = true;

    let exit = runtime
        .block_on(validate_endpoint(&provider))
        .expect_err("a private literal must be refused before any socket opens");

    use std::process::Termination as _;

    assert!(
        matches!(&exit, CliExit::SsrfBlocked(_)),
        "expected SsrfBlocked (70), got: {exit:?}"
    );
    assert_eq!(
        exit.report(),
        std::process::ExitCode::from(crate::cli::error::EXIT_SSRF_BLOCKED)
    );
}
