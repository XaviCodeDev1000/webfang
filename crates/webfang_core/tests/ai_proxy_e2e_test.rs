#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Integration test: AI egress through an explicit forward proxy (ADR-0021 Slice 4, #2551).
//!
//! The wiremock server plays the forward proxy. For `http://` destinations the
//! client speaks absolute-form (`POST http://ai.invalid/v1/embeddings`), so the
//! journal is direct evidence of what the guard routed. No CONNECT coverage:
//! `https://` destinations would tunnel, which this harness does not model.
//!
//! Pinned here, at the CLI boundary:
//! - ambient `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` never reach the AI path;
//! - a configured proxy without its own loopback opt-in is rejected pre-socket;
//! - with the opt-in, the request routes through the proxy in absolute-form;
//! - a redirect into a forbidden literal stops before a second proxy request;
//! - the 16 MiB response cap and the `429` + `Retry-After` pacing both hold
//!   through the proxy hop.

#[path = "common/cli_harness.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::{mount_single_page_site, spawn_budgeted, std_cmd, BehavioralTest};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The provider id every config in this file declares.
const PROVIDER_ID: &str = "e2e-embeddings";

/// Minimal wire body the startup probe accepts: one input in, one vector out.
const EMBEDDING_BODY: &str = r#"{"data":[{"index":0,"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":1,"total_tokens":1}}"#;

/// Every proxy env var WebFang must ignore. Ambient values on the developer
/// machine are scrubbed first, then each test opts into the exact ones it needs.
const SCRUBBED_PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

fn scrub_proxy_env(cmd: &mut std::process::Command) {
    for var in SCRUBBED_PROXY_VARS {
        cmd.env_remove(var);
    }
}

fn write_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("create config temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, contents).expect("write hermetic config.toml");
    (dir, path)
}

/// A direct provider: `base_url` is the endpoint the probe must reach with no
/// proxy hop, and `allow_loopback` is what lets it reach the wiremock endpoint.
fn direct_provider_config(base_url: &str) -> String {
    format!(
        r#"
[[providers]]
id = "{PROVIDER_ID}"
display_name = "E2E embeddings"
kind = "open_ai_compatible"
base_url = "{base_url}"
auth = {{ source = "none" }}
capabilities = ["embedding"]
model = "text-embedding-test"
allow_loopback = true
"#
    )
}

/// A provider whose egress is a `[[proxies]]` catalog entry. `loopback_opt_in`
/// controls the proxy entry's own `allow_loopback`: absent, a loopback proxy
/// target is rejected by `ValidatedProxy` before any socket exists.
fn proxied_provider_config(proxy_uri: &str, loopback_opt_in: bool) -> String {
    let opt_in = if loopback_opt_in {
        "allow_loopback = true\n"
    } else {
        ""
    };
    format!(
        r#"
[[providers]]
id = "{PROVIDER_ID}"
display_name = "E2E embeddings"
kind = "open_ai_compatible"
base_url = "http://ai.invalid/v1"
auth = {{ source = "none" }}
capabilities = ["embedding"]
model = "text-embedding-test"
proxy = "e2e-proxy"

[[proxies]]
id = "e2e-proxy"
url = "{proxy_uri}"
{opt_in}"#
    )
}

async fn mount_embeddings(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(EMBEDDING_BODY, "application/json"))
        .mount(server)
        .await;
}

/// Run a plain scrape whose embeddings slot resolves the configured provider,
/// which forces the startup probe (one `POST /v1/embeddings`).
async fn spawn_probe_run(
    t: &BehavioralTest,
    config_path: &Path,
    what: &'static str,
    configure: impl FnOnce(&mut std::process::Command),
) -> std::process::Output {
    let mut cmd = std_cmd();
    cmd.env("WEBFANG_CONFIG", config_path)
        .arg("--url")
        .arg(t.server.uri())
        .arg("--output")
        .arg(t.out.path())
        .arg("--embedding-provider")
        .arg(PROVIDER_ID);
    scrub_proxy_env(&mut cmd);
    configure(&mut cmd);
    spawn_budgeted(what, cmd, None).await
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The AI transport ignores the proxy environment even when it is set to a
/// reachable server: the probe lands on the provider directly (one request)
/// and the ambient proxy journal stays empty. RED while any AI client inherits
/// wreq's `auto_sys_proxy`.
#[tokio::test]
async fn ambient_proxy_env_is_ignored_for_ai_egress() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let embeddings = MockServer::start().await;
    mount_embeddings(&embeddings).await;
    let ambient_proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&ambient_proxy)
        .await;

    let (_config_dir, config_path) =
        write_config(&direct_provider_config(&format!("{}/v1", embeddings.uri())));

    let output = spawn_probe_run(
        &t,
        &config_path,
        "ambient_proxy_env_is_ignored_for_ai_egress",
        |cmd| {
            cmd.env("HTTP_PROXY", ambient_proxy.uri())
                .env("HTTPS_PROXY", ambient_proxy.uri())
                .env("ALL_PROXY", ambient_proxy.uri());
        },
    )
    .await;

    assert_eq!(
        output.status.code(),
        Some(0),
        "a direct probe must succeed with ambient proxy vars present; stderr:\n{}",
        stderr_of(&output)
    );
    assert_eq!(
        embeddings
            .received_requests()
            .await
            .unwrap_or_default()
            .len(),
        1,
        "the probe must reach the provider directly"
    );
    let hijacked = ambient_proxy.received_requests().await.unwrap_or_default();
    assert!(
        hijacked.is_empty(),
        "ambient proxy vars must never route AI egress, but the proxy observed {} request(s)",
        hijacked.len()
    );
}

/// The proxy's loopback permit is its own opt-in: without it the declaration is
/// rejected at startup (exit 78, Spanish diagnostic) and the proxy observes
/// zero requests — the socket never exists.
#[tokio::test]
async fn configured_proxy_without_loopback_opt_in_is_rejected_before_dial() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_raw(EMBEDDING_BODY, "application/json"))
        .mount(&proxy)
        .await;

    let (_config_dir, config_path) = write_config(&proxied_provider_config(&proxy.uri(), false));

    let output = spawn_probe_run(
        &t,
        &config_path,
        "configured_proxy_without_loopback_opt_in_is_rejected_before_dial",
        |_| {},
    )
    .await;

    assert_eq!(
        output.status.code(),
        Some(78),
        "a loopback proxy without its own allow_loopback is a config error; stderr:\n{}",
        stderr_of(&output)
    );
    assert!(
        stderr_of(&output).contains("está prohibido por el guard SSRF"),
        "the diagnostic must name the SSRF guard; stderr:\n{}",
        stderr_of(&output)
    );
    let requests = proxy.received_requests().await.unwrap_or_default();
    assert!(
        requests.is_empty(),
        "validation must fail before the proxy is dialed, got {} request(s)",
        requests.len()
    );
}

/// With the proxy opt-in the probe routes through it: exactly one request, in
/// absolute-form, naming the destination host and path — the wire-level proof
/// that the destination was delegated to the proxy, not resolved locally.
#[tokio::test]
async fn configured_proxy_routes_the_ai_request_in_absolute_form() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let proxy = MockServer::start().await;
    mount_embeddings(&proxy).await;

    let (_config_dir, config_path) = write_config(&proxied_provider_config(&proxy.uri(), true));

    let output = spawn_probe_run(
        &t,
        &config_path,
        "configured_proxy_routes_the_ai_request_in_absolute_form",
        |_| {},
    )
    .await;

    assert_eq!(
        output.status.code(),
        Some(0),
        "the proxied probe must succeed; stderr:\n{}",
        stderr_of(&output)
    );

    let requests = proxy.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        1,
        "the startup probe is exactly one request, got {}",
        requests.len()
    );
    let request = &requests[0];
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(
        request.url.scheme(),
        "http",
        "absolute-form must preserve the destination scheme: {}",
        request.url
    );
    assert_eq!(
        request.url.host_str(),
        Some("ai.invalid"),
        "absolute-form must name the destination host: {}",
        request.url
    );
    assert_eq!(request.url.path(), "/v1/embeddings");
}

/// A proxy response that redirects into a forbidden literal is stopped by the
/// redirect policy before the client emits a second proxy request. The first
/// (and only) request is observed; the terminal status surfaces as exit 78.
#[tokio::test]
async fn redirect_to_metadata_is_stopped_before_a_second_proxy_request() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
        ))
        .mount(&proxy)
        .await;

    let (_config_dir, config_path) = write_config(&proxied_provider_config(&proxy.uri(), true));

    let output = spawn_probe_run(
        &t,
        &config_path,
        "redirect_to_metadata_is_stopped_before_a_second_proxy_request",
        |_| {},
    )
    .await;

    assert_eq!(
        output.status.code(),
        Some(78),
        "the stopped redirect must fail the probe as a config error; stderr:\n{}",
        stderr_of(&output)
    );
    assert!(
        stderr_of(&output).contains("devolvió el estado 302"),
        "the probe must report the observed status, not a transport error; stderr:\n{}",
        stderr_of(&output)
    );
    let requests = proxy.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        1,
        "the forbidden redirect must not produce a second proxy request, got {}",
        requests.len()
    );
}

/// The 16 MiB response ceiling applies through the proxy hop: an oversized
/// body fails the probe with exit 78 and the Spanish cap message, never a
/// successful parse of a truncated body.
#[tokio::test]
async fn oversized_embedding_response_is_capped() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let proxy = MockServer::start().await;
    let oversized = vec![b'x'; 17 * 1024 * 1024];
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(oversized, "application/json"))
        .mount(&proxy)
        .await;

    let (_config_dir, config_path) = write_config(&proxied_provider_config(&proxy.uri(), true));

    let output = spawn_probe_run(
        &t,
        &config_path,
        "oversized_embedding_response_is_capped",
        |_| {},
    )
    .await;

    assert_eq!(
        output.status.code(),
        Some(78),
        "an oversized embedding body must fail the probe; stderr:\n{}",
        stderr_of(&output)
    );
    assert!(
        stderr_of(&output).contains("excede 16 MiB"),
        "the diagnostic must name the cap; stderr:\n{}",
        stderr_of(&output)
    );
}

/// `429` is retried, and the `Retry-After` delay is honored before the replay:
/// two proxy requests and at least two seconds of wall clock. RED if the retry
/// fires immediately or the header is ignored.
#[tokio::test]
async fn retry_after_paces_the_replay_before_it_fires() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
        .up_to_n_times(1)
        .mount(&proxy)
        .await;
    mount_embeddings(&proxy).await;

    let (_config_dir, config_path) = write_config(&proxied_provider_config(&proxy.uri(), true));

    let started = std::time::Instant::now();
    let output = spawn_probe_run(
        &t,
        &config_path,
        "retry_after_paces_the_replay_before_it_fires",
        |_| {},
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(
        output.status.code(),
        Some(0),
        "the paced replay must succeed; stderr:\n{}",
        stderr_of(&output)
    );
    let requests = proxy.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        2,
        "one rejected attempt plus one paced replay, got {}",
        requests.len()
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(1900),
        "Retry-After: 2 must delay the replay by ~2s, measured {elapsed:?}"
    );
}
