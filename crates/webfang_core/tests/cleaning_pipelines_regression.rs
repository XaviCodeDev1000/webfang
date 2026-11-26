#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Regression tests for webfang's 4 HTML cleaning pipelines.
//!
//! These tests document the CURRENT correct behavior so future refactoring
//! (e.g. PR #251 unification attempt) doesn't break pipeline-specific semantics.
//!
//! Each pipeline has a distinct chain and distinct behavioral contract:
//!
//! | Pipeline | Chain | Key behavior |
//! |----------|-------|-------------|
//! | clean.rs (SemanticProcessor) | legible → strip_html_tags (naive) → normalize_whitespace (split_whitespace) | Readability extraction, naive char-by-char tag strip, Unicode-aware whitespace |
//! | bridge.rs (AggressiveProcessor) | clean_html (lol_html) → normalize_whitespace (ASCII state machine) → strip_html_tags (block removal) | Block-level script/style/noscript/svg/math removal via lol_html |
//! | html_cleaner.rs (MCP) | lol_html element handlers → normalize_whitespace (ASCII state machine) | Tag+CSS-selector removal, attribute stripping, semantic HTML preserved |
//! | chunker.rs (AI) | strip_html_tags (naive + '\\n' on '>') → split by '\\n\\n' | Sentence-boundary creation for AI tokenization |
//!
//! Pipeline 4 (AI Chunker) is in `chunker_regression.rs` — wired
//! into `webfang_ai` to avoid circular dependency.

use webfang_core::infrastructure::converter::html_cleaner::clean_html;

// ─── Pipeline 3: html_cleaner.rs (MCP tools) ────────────────────────────────

#[cfg(all(test, not(miri)))] // clean_html -> lol_html rewrite_str: servo_arc Tree-Borrows UB (stylo#210); RED run aborted on every test in this module
mod html_cleaner_regression {
    use super::*;

    // --- Tag removal ---

    #[test]
    fn scripts_removed_completely() {
        let html = r#"<p>Hello</p><script>document.write("evil")</script><p>World</p>"#;
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("evil"),
            "script content must be removed: {cleaned}"
        );
        assert!(
            cleaned.contains("Hello") && cleaned.contains("World"),
            "visible text preserved: {cleaned}"
        );
    }

    #[test]
    fn style_tags_removed() {
        let html = r#"<p>Content</p><style>.hidden { display: none }</style>"#;
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("display: none"),
            "style content removed: {cleaned}"
        );
        assert!(cleaned.contains("Content"), "text preserved: {cleaned}");
    }

    #[test]
    fn noscript_removed() {
        let html = "<p>Text</p><noscript><p>Enable JS</p></noscript>";
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("Enable JS"),
            "noscript removed: {cleaned}"
        );
        assert!(cleaned.contains("Text"), "text preserved: {cleaned}");
    }

    #[test]
    fn svg_removed() {
        let html = "<p>Text</p><svg><circle r='10'/></svg>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("circle"), "svg removed: {cleaned}");
        assert!(cleaned.contains("Text"), "text preserved: {cleaned}");
    }

    #[test]
    fn nav_header_footer_aside_removed() {
        let html = "<nav>Menu</nav><header>Banner</header><p>Article</p><footer>Footer</footer><aside>Sidebar</aside>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("Menu"), "nav removed: {cleaned}");
        assert!(!cleaned.contains("Banner"), "header removed: {cleaned}");
        assert!(!cleaned.contains("Footer"), "footer removed: {cleaned}");
        assert!(!cleaned.contains("Sidebar"), "aside removed: {cleaned}");
        assert!(
            cleaned.contains("Article"),
            "article content preserved: {cleaned}"
        );
    }

    #[test]
    fn form_iframe_object_embed_removed() {
        // <embed> is a void element (no closing tag) — use self-closing syntax
        let html = "<form>Input</form><iframe>Frame</iframe><object>Obj</object><embed src=\"x.swf\"/><p>Real</p>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("Input"), "form removed: {cleaned}");
        assert!(!cleaned.contains("Frame"), "iframe removed: {cleaned}");
        assert!(!cleaned.contains("Obj"), "object removed: {cleaned}");
        assert!(cleaned.contains("Real"), "text preserved: {cleaned}");
    }

    // --- CSS selector removal ---

    #[test]
    fn css_class_selectors_removed() {
        let html = r#"<div class="site-title">Title</div><div class="global-nav">Nav</div><div class="search">Search</div><p>Content</p>"#;
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("Nav"), "global-nav removed: {cleaned}");
        assert!(!cleaned.contains("Search"), "search removed: {cleaned}");
        assert!(cleaned.contains("Content"), "content preserved: {cleaned}");
    }

    #[test]
    fn aria_hidden_elements_removed() {
        let html = r#"<div aria-hidden="true">Screen reader skip</div><p>Visible</p>"#;
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("Screen reader skip"),
            "aria-hidden removed: {cleaned}"
        );
        assert!(cleaned.contains("Visible"), "visible preserved: {cleaned}");
    }

    // --- Attribute stripping ---

    #[test]
    fn preserved_attributes_kept() {
        let html = r#"<a href="/link" class="btn" data-track="click">Link</a>"#;
        let cleaned = clean_html(html);
        assert!(cleaned.contains("href"), "href preserved: {cleaned}");
        assert!(cleaned.contains("class"), "class preserved: {cleaned}");
        // data-track is NOT in PRESERVED_ATTRS
        assert!(
            !cleaned.contains("data-track"),
            "data-track stripped: {cleaned}"
        );
    }

    // --- Semantic HTML preserved (NOT stripped) ---

    #[test]
    fn output_still_has_semantic_tags() {
        let html = "<h1>Title</h1><p>Paragraph</p><h2>Subtitle</h2><p>More</p>";
        let cleaned = clean_html(html);
        assert!(
            cleaned.contains("<h1>") || cleaned.contains("<h1 "),
            "h1 tag preserved: {cleaned}"
        );
        assert!(
            cleaned.contains("<p>") || cleaned.contains("<p "),
            "p tag preserved: {cleaned}"
        );
        assert!(
            cleaned.contains("<h2>") || cleaned.contains("<h2 "),
            "h2 tag preserved: {cleaned}"
        );
    }

    // --- Whitespace normalization (ASCII state machine) ---

    #[test]
    fn whitespace_collapsed_to_single_space() {
        let html = "<p>  Hello   \n\n  World  </p>";
        let cleaned = clean_html(html);
        // ASCII state machine: consecutive whitespace → single space
        assert!(!cleaned.contains("  "), "no double spaces: {cleaned}");
        assert!(
            cleaned.contains("Hello") && cleaned.contains("World"),
            "text preserved: {cleaned}"
        );
    }

    // --- Edge cases ---

    #[test]
    fn empty_html_returns_empty() {
        assert_eq!(clean_html(""), "");
    }

    #[test]
    fn plain_text_preserved() {
        let html = "Just plain text without any tags";
        let cleaned = clean_html(html);
        assert_eq!(cleaned.trim(), html);
    }
}

// ─── Pipeline 2: bridge.rs (AggressiveProcessor — download pipeline) ─────────
//
// The bridge pipeline chains: clean_html (lol_html) → normalize_whitespace
// (ASCII state machine) → strip_html_tags (block removal of script/style).
// We test through the public CpuBridge::dispatch_resource API.

#[cfg(all(test, not(miri)))] // CpuBridge/rayon spawn + clean_html servo_arc Tree-Borrows UB (stylo#210); RED run aborted on the first test
mod bridge_aggressive_regression {
    use std::sync::Arc;
    use webfang_core::infrastructure::bridge::CpuBridge;
    use webfang_core::infrastructure::content_processing::AggressiveProcessor;
    use webfang_core::infrastructure::cpu_pool::RayonCpuPool;
    use webfang_core::infrastructure::crawler::resource_downloader::DownloadedResource;

    fn resource(html: &str) -> DownloadedResource {
        DownloadedResource {
            url: "https://test.example.com/page".into(),
            bytes: html.as_bytes().to_vec(),
            content_type: Some("text/html".into()),
            size_bytes: html.len() as u64,
        }
    }

    async fn cleaned_text(html: &str) -> String {
        let pool = RayonCpuPool::new(2).expect("build rayon pool");
        let bridge = CpuBridge::new(pool, Arc::new(AggressiveProcessor));
        let rx = bridge.dispatch_resource(resource(html));
        let resource = rx
            .await
            .expect("oneshot must not close")
            .expect("cleaning must succeed");
        resource
            .chunks
            .first()
            .expect("at least one chunk")
            .content
            .clone()
    }

    #[tokio::test]
    async fn script_blocks_removed() {
        let text = cleaned_text("<p>Hello</p><script>alert('xss')</script><p>World</p>").await;
        assert!(!text.contains("alert"), "script body removed: {text}");
        assert!(
            text.contains("Hello") && text.contains("World"),
            "visible text preserved: {text}"
        );
    }

    #[tokio::test]
    async fn style_blocks_removed() {
        let text = cleaned_text("<p>Content</p><style>.red{color:red}</style>").await;
        assert!(!text.contains("color:red"), "style content removed: {text}");
        assert!(text.contains("Content"), "text preserved: {text}");
    }

    #[tokio::test]
    async fn noscript_blocks_removed() {
        let text = cleaned_text("<p>Text</p><noscript>Please enable JS</noscript>").await;
        assert!(
            !text.contains("enable JS") && !text.contains("Enable JS"),
            "noscript removed: {text}"
        );
        assert!(text.contains("Text"), "text preserved: {text}");
    }

    #[tokio::test]
    async fn normal_paragraphs_preserved() {
        let text = cleaned_text("<p>First paragraph.</p><p>Second paragraph.</p>").await;
        assert!(
            text.contains("First paragraph"),
            "first para preserved: {text}"
        );
        assert!(
            text.contains("Second paragraph"),
            "second para preserved: {text}"
        );
    }

    #[tokio::test]
    async fn no_raw_tags_in_output() {
        let text = cleaned_text("<p>Hello <b>bold</b> <i>italic</i> world</p>").await;
        assert!(!text.contains('<'), "no raw HTML tags in output: {text}");
    }

    #[tokio::test]
    async fn nav_header_footer_removed_by_lol_html() {
        let text = cleaned_text(
            "<nav>Menu</nav><header>Top</header><p>Article</p><footer>Bottom</footer>",
        )
        .await;
        assert!(!text.contains("Menu"), "nav removed: {text}");
        assert!(!text.contains("Top"), "header removed: {text}");
        assert!(!text.contains("Bottom"), "footer removed: {text}");
        assert!(text.contains("Article"), "article preserved: {text}");
    }

    #[tokio::test]
    async fn svg_removed() {
        let text = cleaned_text("<p>Text</p><svg><circle r='5'/></svg>").await;
        assert!(!text.contains("circle"), "svg removed: {text}");
        assert!(text.contains("Text"), "text preserved: {text}");
    }
}

// ─── Pipeline 1: clean.rs (SemanticProcessor — pipeline scraper) ─────────────
//
// The clean pipeline chains: extract_readability (legible) → strip_html_tags
// (naive char-by-char) → normalize_whitespace (split_whitespace).
// We test through the public PipelineStage trait on CleanStage.

#[cfg(test)]
mod clean_semantic_regression {
    use webfang_core::application::pipeline::{
        CleanStage, PipelineStage, ScrapedItem, StageOutcome,
    };
    use webfang_core::infrastructure::content_processing::SemanticProcessor;

    async fn run_clean(raw_html: &str) -> String {
        let stage = CleanStage(Box::new(SemanticProcessor));
        let item = ScrapedItem {
            raw_html: raw_html.to_string(),
            ..Default::default()
        };
        match stage.process(item).await {
            StageOutcome::Continue(item) => item.text_content.unwrap_or_default(),
            other => panic!("CleanStage returned non-Continue: {other:?}"),
        }
    }

    #[tokio::test]
    async fn boilerplate_stripped_by_readability() {
        let html = r#"
        <html>
        <head><title>Page</title></head>
        <body>
        <nav>Navigation menu with lots of links</nav>
        <header>Site header banner</header>
        <article>
            <h1>Article Title</h1>
            <p>This is the main content that should be extracted by readability.</p>
        </article>
        <footer>Copyright 2024</footer>
        </body>
        </html>"#;
        let text = run_clean(html).await;
        assert!(
            text.contains("main content"),
            "article content extracted: {text}"
        );
    }

    #[tokio::test]
    async fn nested_tags_produce_clean_text() {
        let html = "<div><p>Hello <b>world</b> <i>italic</i></p></div>";
        let text = run_clean(html).await;
        // naive strip_html_tags: char-by-char, only strips < >
        assert!(
            !text.contains('<') && !text.contains('>'),
            "no HTML tags in output: {text}"
        );
        assert!(
            text.contains("Hello") && text.contains("world"),
            "text content preserved: {text}"
        );
    }

    #[tokio::test]
    async fn whitespace_normalized() {
        let html = "<div>  Hello   \n\n  World  </div>";
        let text = run_clean(html).await;
        // split_whitespace().join(" ") — collapses all runs
        assert!(!text.contains("  "), "no double spaces: {text}");
        assert!(
            text.contains("Hello") && text.contains("World"),
            "text preserved: {text}"
        );
    }

    #[tokio::test]
    async fn short_content_tags_spa() {
        let html = "<html><head></head><body></body></html>";
        let item = {
            let stage = CleanStage(Box::new(SemanticProcessor));
            let item = ScrapedItem {
                raw_html: html.to_string(),
                ..Default::default()
            };
            match stage.process(item).await {
                StageOutcome::Continue(item) => item,
                other => panic!("unexpected: {other:?}"),
            }
        };
        // Short content should flag potential SPA
        assert_eq!(
            item.metadata.get("potential_spa").map(String::as_str),
            Some("true"),
            "short content tagged as potential SPA"
        );
    }

    #[tokio::test]
    async fn metadata_records_sizes() {
        let html = "<p>Hello world content that is long enough to pass the minimum text length threshold.</p>";
        let item = {
            let stage = CleanStage(Box::new(SemanticProcessor));
            let item = ScrapedItem {
                raw_html: html.to_string(),
                ..Default::default()
            };
            match stage.process(item).await {
                StageOutcome::Continue(item) => item,
                other => panic!("unexpected: {other:?}"),
            }
        };
        assert!(
            item.metadata.contains_key("original_size"),
            "original_size recorded"
        );
        assert!(
            item.metadata.contains_key("cleaned_size"),
            "cleaned_size recorded"
        );
        assert!(
            item.metadata.contains_key("reduction_pct"),
            "reduction_pct recorded"
        );
    }
}

// ─── ContentProcessor trait regression ────────────────────────────────────────
//
// These tests prove the trait adapters produce identical behavior to the
// original pipeline functions. Same HTML inputs, same expected outputs.

#[cfg(test)]
mod content_processor_regression {
    use webfang_core::domain::content_processor::ContentProcessor;
    use webfang_core::infrastructure::content_processing::{
        AggressiveProcessor, McpProcessor, SemanticProcessor,
    };

    // ─── SemanticProcessor vs clean.rs ──────────────────────────────────────

    #[test]
    fn semantic_strips_tags() {
        let p = SemanticProcessor;
        let result = p.process("<div><p>Hello <b>world</b></p></div>");
        assert!(!result.contains('<'), "no tags: {result}");
        assert!(
            result.contains("Hello") && result.contains("world"),
            "text preserved: {result}"
        );
    }

    #[test]
    fn semantic_normalizes_whitespace() {
        let p = SemanticProcessor;
        let result = p.process("<div>  Hello   \n\n  World  </div>");
        assert!(!result.contains("  "), "no double spaces: {result}");
        assert!(
            result.contains("Hello") && result.contains("World"),
            "text preserved: {result}"
        );
    }

    #[test]
    fn semantic_empty_returns_empty() {
        let p = SemanticProcessor;
        assert_eq!(p.process(""), "");
    }

    #[test]
    fn semantic_plain_text_preserved() {
        let p = SemanticProcessor;
        let result = p.process("just plain text");
        assert!(
            result.contains("just plain text"),
            "plain text preserved: {result}"
        );
    }

    // ─── AggressiveProcessor vs bridge.rs ───────────────────────────────────

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn aggressive_removes_script_blocks() {
        let p = AggressiveProcessor;
        let result = p.process("<p>Hello</p><script>alert('xss')</script><p>World</p>");
        assert!(!result.contains("alert"), "script body removed: {result}");
        assert!(
            result.contains("Hello") && result.contains("World"),
            "visible text preserved: {result}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn aggressive_removes_style_blocks() {
        let p = AggressiveProcessor;
        let result = p.process("<p>Content</p><style>.red{color:red}</style>");
        assert!(
            !result.contains("color:red"),
            "style content removed: {result}"
        );
        assert!(result.contains("Content"), "text preserved: {result}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn aggressive_removes_boilerplate() {
        let p = AggressiveProcessor;
        let result =
            p.process("<nav>Menu</nav><header>Top</header><p>Article</p><footer>Bottom</footer>");
        assert!(!result.contains("Menu"), "nav removed: {result}");
        assert!(!result.contains("Top"), "header removed: {result}");
        assert!(!result.contains("Bottom"), "footer removed: {result}");
        assert!(result.contains("Article"), "article preserved: {result}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn aggressive_no_raw_tags() {
        let p = AggressiveProcessor;
        let result = p.process("<p>Hello <b>bold</b> world</p>");
        assert!(!result.contains('<'), "no raw tags: {result}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn aggressive_empty_returns_empty() {
        let p = AggressiveProcessor;
        assert_eq!(p.process(""), "");
    }

    // ─── McpProcessor vs html_cleaner.rs ────────────────────────────────────

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_removes_scripts() {
        let p = McpProcessor;
        let result = p.process(r#"<p>Hello</p><script>evil()</script><p>World</p>"#);
        assert!(!result.contains("evil"), "script removed: {result}");
        assert!(
            result.contains("Hello") && result.contains("World"),
            "text preserved: {result}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_preserves_semantic_tags() {
        let p = McpProcessor;
        let result = p.process("<h1>Title</h1><p>Paragraph</p>");
        assert!(
            result.contains("<h1>") || result.contains("<h1 "),
            "h1 preserved: {result}"
        );
        assert!(
            result.contains("<p>") || result.contains("<p "),
            "p preserved: {result}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_removes_boilerplate() {
        let p = McpProcessor;
        let result = p.process("<nav>Menu</nav><main>Content</main><footer>Footer</footer>");
        assert!(!result.contains("Menu"), "nav removed: {result}");
        assert!(!result.contains("Footer"), "footer removed: {result}");
        assert!(result.contains("Content"), "content preserved: {result}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_empty_returns_empty() {
        let p = McpProcessor;
        assert_eq!(p.process(""), "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_css_selectors_removed() {
        let p = McpProcessor;
        let result = p.process(
            r#"<div class="site-title">Title</div><div class="global-nav">Nav</div><p>Content</p>"#,
        );
        assert!(!result.contains("Nav"), "global-nav removed: {result}");
        assert!(result.contains("Content"), "content preserved: {result}");
    }

    // ─── Cross-processor behavioral divergence tests ────────────────────────
    //
    // These tests document that different processors intentionally produce
    // different outputs for the same input — this is the whole point.

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc UB via clean_html (stylo#210)")]
    fn mcp_preserves_tags_while_aggressive_strips() {
        let html = "<p>Hello <b>world</b></p>";
        let mcp = McpProcessor;
        let aggressive = AggressiveProcessor;
        let mcp_result = mcp.process(html);
        let agg_result = aggressive.process(html);
        // MCP keeps semantic tags
        assert!(
            mcp_result.contains("<p>") || mcp_result.contains("<p "),
            "MCP keeps <p>: {mcp_result}"
        );
        // Aggressive strips all tags
        assert!(
            !agg_result.contains('<'),
            "Aggressive strips all: {agg_result}"
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "aggressive path reaches clean_html servo_arc UB (stylo#210)"
    )]
    fn semantic_uses_readability_while_aggressive_uses_lol_html() {
        // A complex page where Readability extraction differs from lol_html stripping
        let html = r#"<html><head><title>Page</title></head>
        <body>
        <nav>Navigation menu</nav>
        <article>
            <h1>Article Title</h1>
            <p>Main content paragraph with enough text to be extracted by readability.</p>
        </article>
        <footer>Copyright notice</footer>
        </body></html>"#;
        let semantic = SemanticProcessor;
        let aggressive = AggressiveProcessor;
        let sem_result = semantic.process(html);
        let agg_result = aggressive.process(html);
        // Both should preserve the article content
        assert!(
            sem_result.contains("Main content") || sem_result.contains("Article Title"),
            "semantic extracts content: {sem_result}"
        );
        assert!(
            agg_result.contains("Main content") || agg_result.contains("Article Title"),
            "aggressive preserves content: {agg_result}"
        );
    }
}

// ─── Issue #3009: embedding boundary must never receive raw block bodies ──

#[cfg(test)]
mod embedding_boundary_regression {
    use std::any::Any;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use webfang_core::domain::embedding_port::EmbeddingPort;
    use webfang_core::domain::semantic_cleaner::SemanticCleaner;
    use webfang_core::error::SemanticError;
    use webfang_core::infrastructure::semantic_cleaner::SemanticCleanerImpl;
    use webfang_test_utils::redact_nondeterministic;

    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Captures the exact strings handed to `embed_batch` — the embedding boundary.
    #[derive(Default)]
    struct CapturingPort {
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl EmbeddingPort for CapturingPort {
        fn embed<'a>(&'a self, _text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            Box::pin(async { Ok(vec![1.0, 0.0, 0.0]) })
        }

        fn embed_batch<'a>(
            &'a self,
            texts: &'a [String],
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
            self.batches
                .lock()
                .expect("test capture lock")
                .push(texts.to_vec());
            Box::pin(async move { Ok(vec![vec![1.0, 0.0, 0.0]; texts.len()]) })
        }

        fn embedding_dim(&self) -> usize {
            3
        }

        fn model_id(&self) -> &str {
            "capturing-test-port"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// #3009: when `legible` finds no article the cleaner degrades to a tag
    /// stripper. That fallback must drop script/style/noscript bodies before
    /// any text reaches `embed_batch`.
    #[tokio::test]
    #[cfg_attr(miri, ignore = "tokio runtime socketpair unsupported by Miri")]
    async fn degraded_stripper_drops_script_style_noscript_before_embed_batch() {
        // No readable article: `legible::parse` returns Err and the fallback runs.
        let html = r#"<html><head><title>x</title></head><body><script>LEAK_SCRIPT</script><style>LEAK_STYLE</style><noscript>LEAK_NOSCRIPT</noscript></body></html>"#;
        let port = Arc::new(CapturingPort::default());
        let cleaner = SemanticCleanerImpl::new(port.clone(), 1000, 0.3);
        let _ = cleaner
            .clean("https://example.test", html)
            .await
            .expect("clean must succeed on degraded input");

        let batches = port.batches.lock().expect("test capture lock");
        let emitted: Vec<&String> = batches.iter().flatten().collect();
        assert!(!emitted.is_empty(), "the fallback must still emit text");
        for text in &emitted {
            assert!(
                !text.contains("LEAK_SCRIPT"),
                "script body reached embedding: {text}"
            );
            assert!(
                !text.contains("LEAK_STYLE"),
                "style body reached embedding: {text}"
            );
            assert!(
                !text.contains("LEAK_NOSCRIPT"),
                "noscript body reached embedding: {text}"
            );
        }
        insta::assert_snapshot!(redact_nondeterministic(
            std::path::Path::new("/webfang-no-temp-path"),
            &format!("{emitted:?}")
        ));
    }
}
