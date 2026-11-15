use super::*;
use crate::mcp_server::handlers::test_support::result_text;
/// #1613 EC-06: `discover_urls` / `discover_sitemap` share
/// `discovered_urls_tool_result` — a serialization failure there is a
/// TOOL ERROR, not a success body reading `"failed to serialize"`.
#[test]
fn discovered_urls_serialization_failure_is_a_tool_error() {
    let unwritable = Err(serde_json::Error::io(std::io::Error::other(
        "url set not serializable",
    )));
    let res = discovered_urls_tool_result("https://example.com", unwritable);
    assert_eq!(
        res.is_error,
        Some(true),
        "a lost URL list must be isError:true, got: {}",
        result_text(&res)
    );
    let text = result_text(&res);
    assert!(
        text.contains("no se pudo serializar la lista de URLs"),
        "the honest Spanish reason must reach the agent, got: {text}"
    );
}

/// The success arm of the shared mapping is untouched.
#[test]
fn discovered_urls_serialization_success_stays_a_tool_success() {
    let res = discovered_urls_tool_result(
        "https://example.com",
        Ok("[\"https://example.com/a\"]".to_string()),
    );
    assert_ne!(
        res.is_error,
        Some(true),
        "a serializable list is not an error"
    );
    assert!(
        result_text(&res).contains("https://example.com/a"),
        "the payload must survive"
    );
}
