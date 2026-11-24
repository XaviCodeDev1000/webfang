use super::tests_support::{
    seed_one, test_handler_with_export_roots, test_handler_with_robots, test_state, vu,
};
use super::*;
use crate::mcp_server::handlers::test_support::result_text;
use serial_test::serial;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn process_export_pipeline_seeded_writes_to_output_dir() {
    let (handler, _tmp) = test_handler_with_export_roots().await;
    seed_one(&handler);
    let res = handler
        .process_export_pipeline(Parameters(ProcessExportPipelineParams {
            url: None,
            pipeline_format: Some("jsonl".to_string()),
        }))
        .await
        .expect("process_export_pipeline returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("Export completed"),
        "pipeline export must report completion: {text}"
    );
}

/// XP-P-08/G-9 (issue #1608): the pipeline's write target — the
/// container's configured `output_dir` — is gated against the export
/// roots at request time. An out-of-roots directory must FAIL with the
/// gate's invalid-params error, not just warn (the old #769-only
/// behavior), and no export file may be written.
#[tokio::test]
async fn process_export_pipeline_rejects_output_dir_outside_roots() {
    let (state, tmp) = test_state().await;
    let other_root = TempDir::new().expect("create unrelated root temp dir");
    let state = state.with_export_roots(vec![other_root.path().to_path_buf()]);
    let handler = McpHandler::new(state);
    seed_one(&handler);

    let res = handler
        .process_export_pipeline(Parameters(ProcessExportPipelineParams {
            url: None,
            pipeline_format: Some("jsonl".to_string()),
        }))
        .await;
    let err = res.expect_err("out-of-roots configured output_dir must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("outside allowed export roots"),
        "rejection must come from the export-root gate, got: {msg}"
    );
    assert!(
        !tmp.path().join("export.jsonl").exists(),
        "no export file may be written outside the declared roots"
    );
}

/// Issue #605 regression: when `url` is provided, the pipeline must scrape
/// it live instead of reading persisted results. With no network/seed this
/// surfaces as a scrape (network) error — never the persisted-only
/// "no hay resultados disponibles para exportar" message, which would
/// prove the `url` argument was ignored.
#[tokio::test]
async fn process_export_pipeline_with_url_invokes_scrape() {
    let (handler, _tmp) = test_handler_with_export_roots().await;
    let res = handler
        .process_export_pipeline(Parameters(ProcessExportPipelineParams {
            url: Some(vu("https://quotes.toscrape.com")),
            pipeline_format: Some("jsonl".to_string()),
        }))
        .await
        .expect("process_export_pipeline returns Ok on scrape failure");
    let text = result_text(&res);
    assert!(
        !text.contains("no hay resultados disponibles para exportar"),
        "url branch must not fall back to persisted 'no results': {text}"
    );
    assert!(
        text.contains("error al rastrear") || text.contains("Export completed"),
        "url branch must attempt a live scrape: {text}"
    );
}

/// #749: the url branch must respect robots.txt — a disallowed URL fails
/// with the branch's error wrapper mentioning `robots.txt`, and no page
/// fetch is issued (only the robots.txt probe reaches the mock).
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial] // WEBFANG_MCP_DISABLE_SSRF is process-global — see scraping.rs
async fn process_export_pipeline_url_robots_disallowed_errors_before_scrape() {
    // Wiremock binds 127.0.0.1 — lift BOTH guards for this test only: the MCP
    // entry validator and the shared core literal-IP entry guard (F-06 + F-32,
    // #1217). Otherwise the robots gate is satisfied by an SSRF short-circuit
    // instead of real rules (#1301). EnvGuard restores the originals on drop,
    // so the "1"s cannot leak into sibling tests in a shared process (#1126).
    // The SSRF guard itself is asserted by the dedicated regression test below.
    //
    // NOTE (issue #1885): this robots test keeps its own handler and mock
    // setup — unlike the ai/scraping robots tests it needs export roots
    // covering the fixture's output dir (XP-P-08/G-9, #1608), so only the
    // guard is shared, via the canonical robots-chain constructor.
    let _guard = webfang_test_utils::EnvGuard::wiremock_robots();
    let (handler, _tmp) = test_handler_with_robots().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private\n"),
        )
        .mount(&server)
        .await;

    let res = handler
        .process_export_pipeline(Parameters(ProcessExportPipelineParams {
            url: Some(vu(&format!("{}/private/page", server.uri()))),
            pipeline_format: Some("jsonl".to_string()),
        }))
        .await
        .expect("process_export_pipeline returns Ok on robots denial");

    let json = serde_json::to_value(&res).expect("CallToolResult serializes");
    assert_eq!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "robots denial must set isError:true, got: {json}"
    );
    let text = result_text(&res);
    assert!(
        text.contains("error al rastrear") && text.contains("robots.txt"),
        "url branch must wrap the robots denial: {text}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    let robots_hits = requests
        .iter()
        .filter(|r| r.url.path() == "/robots.txt")
        .count();
    let page_hits = requests
        .iter()
        .filter(|r| r.url.path() != "/robots.txt")
        .count();
    // The denial must come from real robots rules, not from a pre-fetch
    // short-circuit that also happens to issue no page request. Exactly one
    // robots.txt fetch per domain is the #794 single-flight invariant.
    assert_eq!(
        robots_hits, 1,
        "robots rules must actually be consulted before the denial"
    );
    assert_eq!(page_hits, 0, "the robots gate must block the page fetch");
}

/// #749 fold-in regression: the url branch now validates against SSRF —
/// a loopback target is rejected with an SSRF protocol error before any
/// fetch. Mirrors the loopback tests of the other URL-fetching tools
/// (Bug #673).
#[tokio::test]
#[serial]
async fn process_export_pipeline_url_ssrf_guard_blocks_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    let (handler, _tmp) = test_handler_with_export_roots().await;
    let res = handler
        .process_export_pipeline(Parameters(ProcessExportPipelineParams {
            url: Some(vu("http://127.0.0.1/")),
            pipeline_format: Some("jsonl".to_string()),
        }))
        .await;
    assert!(res.is_err(), "loopback URL must be a protocol error");
    let msg = res
        .expect_err("loopback URL must be blocked by SSRF")
        .to_string();
    assert!(
        msg.contains("SSRF"),
        "error must report SSRF protection, got: {msg}"
    );
}
