//! RAG export behavioral tests — vector export header invariants (issue #502)
//! and the F-31 credential-leak pin (issue #1233).

use crate::assert_snapshot_redacted;
use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// The basic-auth pair embedded in the requested URL. `secretpass` is the canary:
/// it must not appear in any artefact the run writes.
const CREDENTIAL: &str = "user:secretpass";

/// The password half of [`CREDENTIAL`], asserted separately so a leak that
/// survives percent-encoding or partial redaction is still caught.
const PASSWORD: &str = "secretpass";

/// The harness authority, re-spelled with credentials in the userinfo slot.
///
/// `wiremock` binds `127.0.0.1` on an ephemeral port and serves plain HTTP, so
/// the credentials never reach the wire (the HTTP request target is origin-form)
/// — they only exercise WebFang's own URL handling and output paths.
fn credentialed_url(t: &BehavioralTest) -> String {
    let authority = t
        .server
        .uri()
        .strip_prefix("http://")
        .expect("wiremock binds http")
        .to_string();
    format!("http://{CREDENTIAL}@{authority}/")
}

const PAGE_HTML: &str = r#"
<html><head><title>Vector Export Test</title></head>
<body><main><article>
<h1>Export Me</h1>
<p>Enough content for the extractor to produce a real document chunk.</p>
<p>A second paragraph keeps readability extraction stable and deterministic.</p>
</article></main></body></html>
"#;

/// Issue #502 repro: the vector export header `total_documents` must equal
/// the number of entries actually present in the `documents` array.
///
/// Semantic JSON assertions instead of snapshots on purpose: the header embeds
/// a `created_at` timestamp that is non-deterministic by design.
///
/// `--export-format vector` needs `--clean-ai` (preflight #796) and, since
/// ADR-0004 retired the local ONNX stack, `--clean-ai` needs a CONFIGURED
/// REMOTE embedding provider. The provider is the test's own wiremock
/// endpoint (`common::embeddings`), so the test is hermetic: no endpoint, no
/// credential and no network beyond the loopback interface.
#[tokio::test]
async fn vector_export_total_documents_matches_documents() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    crate::common::embeddings::mount_embeddings(&t.server).await;
    let provider_config = crate::common::embeddings::provider_config(&t.server);

    t.scraper_cmd()
        .env("WEBFANG_CONFIG", &provider_config)
        .arg("--embedding-provider")
        .arg(crate::common::embeddings::PROVIDER_ID)
        .arg("--single-page")
        .arg("--export-format")
        .arg("vector")
        .arg("--clean-ai")
        .arg("--quiet")
        .assert()
        .success();

    let export_path = t.out.path().join("export.json");
    let content = std::fs::read_to_string(&export_path).expect("vector export file should exist");
    let json: serde_json::Value =
        serde_json::from_str(&content).expect("export must be valid JSON");

    let total = json["total_documents"]
        .as_u64()
        .expect("total_documents must be a number");
    let documents = json["documents"]
        .as_array()
        .expect("documents must be an array");

    assert!(
        !documents.is_empty(),
        "at least one document should be exported"
    );
    assert_eq!(
        total,
        documents.len() as u64,
        "header total_documents must match the documents array length"
    );
}

/// AUDIT-01 F-31 (#1233): URL credentials must never reach the exported
/// Markdown.
///
/// `ValidUrl::new` and `impl From<url::Url> for ValidUrl` were two unhardened
/// construction doors: a caller that wrapped an already-parsed `url::Url`
/// bypassed the #675-5 credential strip, so `user:secretpass@host` flowed
/// into `ScrapedContent.url` — and from there into the Markdown frontmatter.
/// Both doors are now closed (`new` is crate-private and test-only, `From`
/// is deleted, `TryFrom` is hardened) and every production wrapper goes
/// through `try_from_url`.
///
/// Drives the real binary end-to-end rather than the value object, because
/// the leak was a *composition* bug: each individual layer was correct.
#[tokio::test]
async fn exported_markdown_never_leaks_url_credentials() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    cmd()
        .arg("--url")
        .arg(credentialed_url(&t))
        .arg("--output")
        .arg(t.out.path())
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let markdown = t.read_md_content();
    assert!(
        !markdown.contains(CREDENTIAL),
        "F-31: credentials leaked into the exported Markdown:\n{markdown}"
    );
    assert!(
        !markdown.contains(PASSWORD),
        "F-31: the password alone leaked into the exported Markdown:\n{markdown}"
    );

    // Pin the observable shape too: the frontmatter `url:` line is where the
    // leak was visible, so the snapshot is the regression guard on the fix.
    assert_snapshot_redacted(
        "exported_markdown_never_leaks_url_credentials",
        t.out.path(),
        &markdown,
    );
}

/// The same invariant for the `--trace-file` JSONL.
///
/// Regression test for #1239 (F-31b): the raw CLI seed URL used to reach the
/// trace through `#[instrument(fields(url = %opts.url))]` on `orchestrator::run`
/// plus the per-request downloader spans, because `CrawlOptions.url` was a raw
/// `url::Url` built straight from argv by `cli::args::url_from_args` and no
/// `ValidUrl` was constructed anywhere on that path. The fix parses the seed
/// URL into `ValidUrl` at the argv boundary (the same hardening the MCP side
/// already applies via `McpUrl`, #1116), so every `url` field in the trace
/// renders the credential-stripped URL.
#[tokio::test]
async fn trace_file_never_leaks_url_credentials() {
    let t = BehavioralTest::new().await;
    // The trace lives outside the export dir so the Markdown walk cannot see it.
    let trace_dir = tempfile::TempDir::new().expect("trace temp dir");
    let trace_path = trace_dir.path().join("trace.jsonl");

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    cmd()
        .arg("--url")
        .arg(credentialed_url(&t))
        .arg("--output")
        .arg(t.out.path())
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let trace = std::fs::read_to_string(&trace_path).expect("trace file must be written");
    assert!(
        !trace.contains(CREDENTIAL),
        "F-31: credentials leaked into --trace-file:\n{trace}"
    );
    assert!(
        !trace.contains(PASSWORD),
        "F-31: the password alone leaked into --trace-file:\n{trace}"
    );
}
