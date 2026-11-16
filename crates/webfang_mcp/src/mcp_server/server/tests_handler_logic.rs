//! Tests extraídos del hub `server.rs` por tema (PR 1 de #2584, movimiento puro).
//! Los cuerpos son verbatim del módulo de tests inline original.

use super::tests_common::test_handler;
use crate::mcp_server::{McpState, MCP_TOOL_COUNT};
use webfang_core::config::Config;
use webfang_core::di::Container;
#[tokio::test]
async fn test_handler_builds_with_all_tools() {
    let handler = test_handler().await;
    let tools = handler.tool_router.list_all();
    // The EXACT count, not a floor (#1817): "at least 35" let the registry
    // drift in either direction without failing, and the figure is read
    // from the one place that owns it.
    assert_eq!(
        tools.len(),
        MCP_TOOL_COUNT,
        "the server must advertise exactly {MCP_TOOL_COUNT} tools, got {}",
        tools.len()
    );

    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(tool_names.contains(&"scrape_url"));
    assert!(tool_names.contains(&"validate_url"));
    assert!(tool_names.contains(&"clean_html"));
    assert!(tool_names.contains(&"detect_waf"));
    assert!(tool_names.contains(&"download_assets"));
    assert!(tool_names.contains(&"extract_domain"));
    assert!(tool_names.contains(&"normalize_url"));
    assert!(tool_names.contains(&"convert_html_to_markdown"));
}

/// Test tool logic by calling the underlying functions directly
/// (bypasses MCP protocol layer which requires peer/session setup).

#[test]
fn test_validate_url_logic() {
    let url = url::Url::parse("https://example.com/path?q=1").unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("example.com"));
    assert_eq!(url.path(), "/path");
}

#[test]
fn test_normalize_url_logic() {
    let mut url = url::Url::parse("https://example.com/path/#fragment").unwrap();
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    let result = url.to_string();
    assert!(!result.contains("#fragment"));
    assert!(!result.ends_with("/"));
}

#[test]
fn test_extract_domain_logic() {
    let url = url::Url::parse("https://www.example.com/path").unwrap();
    assert_eq!(url.host_str(), Some("www.example.com"));
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_clean_html_logic() {
    let html = "<html><head><script>alert('x')</script></head><body><p>Hello</p></body></html>";
    let cleaned = webfang_core::infrastructure::converter::html_cleaner::clean_html(html);
    assert!(!cleaned.contains("script"));
    assert!(cleaned.contains("Hello"));
}

#[test]
fn test_convert_html_to_markdown_logic() {
    use webfang_core::domain::ports::MarkdownConverter;
    let html = "<h1>Title</h1><p>Paragraph</p>";
    let md = webfang_core::infrastructure::converter::htmd_converter::HtmdConverter.convert(html);
    assert!(md.contains("Title"));
    assert!(md.contains("Paragraph"));
}

#[test]
fn test_waf_detector_logic() {
    use webfang_core::domain::waf::InspectionContext;
    use webfang_core::infrastructure::http::waf_engine::WafInspector;
    let clean_html = "<html><body>Normal content</body></html>";
    let verdict = WafInspector::inspect(clean_html, &InspectionContext::default());
    assert!(!verdict.is_blocked);
}

#[test]
fn test_waf_detector_cloudflare() {
    use webfang_core::domain::waf::InspectionContext;
    use webfang_core::infrastructure::http::waf_engine::WafInspector;
    let cf_html = "<div id=\"cf-turnstile\" data-sitekey=\"abc123\"></div>";
    let verdict = WafInspector::inspect(cf_html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert!(verdict
        .evidences
        .first()
        .is_some_and(|e| e.provider.contains("Cloudflare")));
}

#[test]
fn test_output_path_logic() {
    let path =
        webfang_core::adapters::url_path::OutputPath::from_url("https://example.com/docs/page")
            .unwrap();
    let full = path.to_full_path();
    assert!(full.contains("example.com"));
    assert!(full.contains("docs"));
}

#[test]
fn test_frontmatter_generation() {
    let fm = webfang_core::infrastructure::output::frontmatter::generate(
        "Test Title",
        "https://example.com",
        None,
        None,
        None,
        &[],
    );
    assert!(fm.contains("Test Title"));
    assert!(fm.contains("example.com"));
}

#[cfg_attr(miri, ignore)]
#[test]
fn test_highlight_code_blocks_logic() {
    let md = "```rust\nfn main() {}\n```";
    let highlighted =
        webfang_core::infrastructure::converter::syntax_highlight::highlight_code_blocks(md);
    // Syntax highlighting may or may not add markup; just verify it returns something
    assert!(!highlighted.is_empty());
}

#[test]
fn test_convert_wiki_links_logic() {
    let md = "https://example.com/page";
    let wikilinks =
        webfang_core::infrastructure::converter::wikilinks::convert_wiki_links(md, "example.com");
    // Wiki link conversion replaces same-domain URLs with [[page]] syntax
    assert!(!wikilinks.is_empty());
}

#[test]
fn test_mcp_state_with_downloader() {
    use std::sync::Arc;

    let config = Config::default();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let container = rt
        .block_on(Container::new(config.crawler, config.scraper))
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let dl_config = webfang_core::adapters::downloader::DownloadConfig {
        output_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let downloader =
        Arc::new(webfang_core::adapters::downloader::Downloader::new(dl_config).unwrap());
    let downloader_clone = downloader.clone();

    let state = McpState::new(container).with_downloader(downloader);
    assert!(
        state.downloader.is_some(),
        "McpState must hold the shared Downloader after with_downloader()"
    );
    assert!(
        Arc::ptr_eq(state.downloader.as_ref().unwrap(), &downloader_clone),
        "with_downloader must store the exact Arc (connection pool identity)"
    );

    // Clone preserves the shared pool
    let state2 = state.clone();
    assert!(
        state2.downloader.is_some(),
        "clone must preserve downloader"
    );
    assert!(
        Arc::ptr_eq(
            state.downloader.as_ref().unwrap(),
            state2.downloader.as_ref().unwrap()
        ),
        "cloned McpState must share the same Downloader Arc"
    );
}
