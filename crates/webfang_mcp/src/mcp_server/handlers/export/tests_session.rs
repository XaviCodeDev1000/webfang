use super::tests_support::seed_content;
use super::*;

/// REQ-MCP-EXPORT-05 (re-pointed to the session source, #1290): when the
/// session holds no results — no crawl has run, or its extraction produced
/// nothing — the loader must return an honest `CallToolResult::error`
/// (isError:true) carrying the Spanish "no hay resultados" message —
/// never a fake success, never a silently empty file.
#[test]
fn load_session_results_empty_session_is_honest_error() {
    let session_results = Arc::new(std::sync::Mutex::new(Vec::new()));
    let err =
        load_session_results(&session_results).expect_err("an empty session must be an error");

    // Serialize exactly as the MCP transport would, then assert the honest
    // error contract: isError:true plus the Spanish message.
    let json = serde_json::to_value(&err).expect("CallToolResult must serialize");
    assert_eq!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "empty-session path must set isError:true, got: {json}"
    );
    let text = json
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    assert!(
        text.contains("no hay resultados disponibles para exportar"),
        "honest Spanish no-results error expected, got: {text}"
    );
}

/// The session read COPIES: two exports after one crawl must serve the
/// same records (the buffer is only replaced by the next crawl run). This
/// replaces the #1122 executor-starvation timing test, whose old disk-scan
/// shape is no longer representable here; the blocking-pool wrapper is the
/// anti-starvation mechanism, while this test pins the snapshot semantics.
#[test]
fn load_session_results_copies_without_draining() {
    let session_results = Arc::new(std::sync::Mutex::new(vec![seed_content(
        "https://example.com/a",
        "A",
        "body a",
    )]));
    let first = load_session_results(&session_results).expect("one item is exportable");
    let second = load_session_results(&session_results).expect("the same item exports again");
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].url, second[0].url);
}
