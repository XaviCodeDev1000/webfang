//! Tests for `DefaultDomInspector` and `NoOpInspector`.
//!
//! Extracted from `dom_inspector.rs` to bring it under the ~500-line
//! file-size rule. Movement is verbatim; imports rewritten from
//! `use super::*` to explicit crate-absolute paths.
//!
//! The Tree Borrows UB / Miri gating comment on `default_dom_inspector_tests`
//! is load-bearing documentation — preserved unchanged.

#[cfg(test)]
mod tests {
    use crate::domain::DomInspectorPort;
    use crate::infrastructure::scraper::dom_inspector::NoOpInspector;

    // --- NoOpInspector tests ---

    #[test]
    fn test_noop_inspector_inspect_returns_default() {
        let inspector = NoOpInspector;
        let document = scraper::Html::parse_document("<html><body><p>hello</p></body></html>");
        let report = inspector.inspect(&document);
        assert_eq!(report.element_count, 0);
        assert!(!report.truncated);
        assert!(report.tag_counts.is_empty());
        assert_eq!(report.max_depth, 0);
        assert!(report.common_classes.is_empty());
        assert!(report.common_ids.is_empty());
    }

    #[test]
    fn test_noop_inspector_suggest_returns_empty() {
        let inspector = NoOpInspector;
        let document = scraper::Html::parse_document("<html><body><p>hello</p></body></html>");
        let suggestions = inspector.suggest(&document, ".nonexistent");
        assert!(suggestions.is_empty());
    }
}

// DefaultDomInspector tests construct and drop `scraper::Selector` values (via
// `Selector::parse` in `collect_candidates`/`inspect`/`suggest`). Dropping a
// selector trips a Tree Borrows UB inside servo_arc's `ArcUnion`
// (`servo_arc` 0.4.3 lib.rs:535, `Arc::drop`) — the same defect that gated
// readability/author_extractor (5a2adc2, #515) — and large DOMs
// (`builder_methods`, 600 divs) spin indefinitely under the interpreter. In CI
// this module hung the whole infra-fast job until the 60m timeout (#751, run
// 31983525568: 31 tests never completed). Regular (non-Miri) runs keep coverage.
#[cfg(all(test, not(miri)))]
mod default_dom_inspector_tests {
    use crate::domain::DomInspectorPort;
    use crate::infrastructure::scraper::dom_inspector::DefaultDomInspector;

    // --- DefaultDomInspector::inspect tests ---

    #[test]
    fn test_default_inspector_counts_tags_correctly() {
        let html = r#"<html>
  <head><title>Test</title></head>
  <body>
    <div class="container main">
      <h1 id="header">Title</h1>
      <p class="text">Paragraph</p>
      <span>Span</span>
    </div>
  </body>
</html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // 8 elements: html, head, title, body, div, h1, p, span
        assert_eq!(report.element_count, 8);
        assert!(!report.truncated);

        // Tag counts
        assert_eq!(*report.tag_counts.get("html").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("head").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("title").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("body").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("div").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("h1").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("p").unwrap_or(&0), 1);
        assert_eq!(*report.tag_counts.get("span").unwrap_or(&0), 1);

        // Classes: container(1), main(1), text(1)
        assert!(!report.common_classes.is_empty());
        let class_names: Vec<&str> = report
            .common_classes
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(class_names.contains(&"container"));
        assert!(class_names.contains(&"main"));
        assert!(class_names.contains(&"text"));

        // IDs: header(1)
        assert!(!report.common_ids.is_empty());
        assert_eq!(report.common_ids[0].0, "header");
        assert_eq!(report.common_ids[0].1, 1);

        // Max depth: html(1), head(2), title(3), body(2), div(3), h1(4), p(4), span(4)
        assert_eq!(report.max_depth, 4);
    }

    #[test]
    fn test_default_inspector_respects_node_cap() {
        // Generate HTML with 20 elements, cap at 10.
        let html = "<html><body>".to_owned()
            + &"<div>".repeat(20)
            + &"</div>".repeat(20)
            + "</body></html>";
        let document = scraper::Html::parse_document(&html);
        let inspector = DefaultDomInspector::new().with_node_cap(10);
        let report = inspector.inspect(&document);

        assert!(report.truncated);
        assert_eq!(report.element_count, 10);
    }

    #[test]
    fn test_default_inspector_empty_html_returns_minimal_report() {
        // The HTML5 parser creates implicit html/head/body even for empty input.
        let document = scraper::Html::parse_document("");
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // Parser creates 3 implicit elements: html, head, body.
        assert_eq!(report.element_count, 3);
        assert!(!report.truncated);
        assert_eq!(report.tag_counts.len(), 3);
        assert!(report.tag_counts.contains_key("html"));
        assert!(report.tag_counts.contains_key("head"));
        assert!(report.tag_counts.contains_key("body"));
        assert!(report.common_classes.is_empty());
        assert!(report.common_ids.is_empty());
    }

    #[test]
    fn test_default_inspector_max_depth_calculation() {
        let inspector = DefaultDomInspector::new();

        // Shallow tree: <div><p>text</p></div>
        let shallow_doc = scraper::Html::parse_fragment("<div><p>text</p></div>");
        let shallow_report = inspector.inspect(&shallow_doc);

        // Deep tree: <div><div><div><div><p>deep</p></div></div></div></div>
        let deep_doc = scraper::Html::parse_fragment(
            "<div><div><div><div><p>deep</p></div></div></div></div>",
        );
        let deep_report = inspector.inspect(&deep_doc);

        // Deeper nesting must produce greater max_depth.
        assert!(
            deep_report.max_depth > shallow_report.max_depth,
            "deep tree max_depth ({}) should exceed shallow tree max_depth ({})",
            deep_report.max_depth,
            shallow_report.max_depth
        );
        // Both should have non-zero depth (at least 1 ancestor: the root node).
        assert!(shallow_report.max_depth >= 1);
        assert!(deep_report.max_depth >= 1);
    }

    #[test]
    fn test_default_inspector_class_frequency_sorted_descending() {
        let html = r#"<html><body>
          <div class="alpha">1</div>
          <div class="alpha">2</div>
          <div class="alpha">3</div>
          <div class="beta">4</div>
          <span class="beta">5</span>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let report = inspector.inspect(&document);

        // alpha appears 3 times, beta appears 2 times
        assert_eq!(report.common_classes.len(), 2);
        assert_eq!(report.common_classes[0].0, "alpha");
        assert_eq!(report.common_classes[0].1, 3);
        assert_eq!(report.common_classes[1].0, "beta");
        assert_eq!(report.common_classes[1].1, 2);
    }

    // --- DefaultDomInspector::suggest tests ---

    #[test]
    fn test_default_inspector_suggest_above_threshold() {
        let html = r#"<html><body>
          <div class="article-body"><p class="article-title">content</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".article-body");

        // ".article-body" is an exact match candidate → score 1.0
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].selector, ".article-body");
        assert!((suggestions[0].score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_inspector_suggest_below_threshold() {
        // Use a selector that shares no similarity with any DOM element
        let html = r#"<html><body>
          <div class="zzzz"><p class="zzzzz">content</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new().with_threshold(0.99);
        let suggestions = inspector.suggest(&document, ".aaaa");

        // With threshold 0.99, only exact matches pass. ".aaaa" ≠ any candidate.
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_default_inspector_suggest_class_selector() {
        let html = r#"<html><body>
          <div class="main-content"><p>text</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main-content");

        // Should include the class selector ".main-content"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&".main-content"),
            "expected .main-content in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_id_selector() {
        let html = r#"<html><body>
          <div id="header"><p>text</p></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, "#header");

        // Should include the id selector "#header"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&"#header"),
            "expected #header in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_tag_selector() {
        let html = r#"<html><body>
          <article><p>text</p></article>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, "article");

        // Should include the bare tag "article"
        let selectors: Vec<&str> = suggestions.iter().map(|s| s.selector.as_str()).collect();
        assert!(
            selectors.contains(&"article"),
            "expected 'article' tag in suggestions, got: {selectors:?}"
        );
    }

    #[test]
    fn test_default_inspector_suggest_sorted_by_score_descending() {
        let html = r#"<html><body>
          <div class="main"><div class="main-content"><div class="main-content-extra">
            <p>text</p>
          </div></div></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main-content");

        // Verify sorted by score descending
        for window in suggestions.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "suggestions not sorted desc: {} >= {}",
                window[0].score,
                window[1].score
            );
        }
    }

    #[test]
    fn test_default_inspector_suggest_truncated_to_top_5() {
        // Many similar candidates
        let html = r#"<html><body>
          <div class="main1"><div class="main2"><div class="main3">
          <div class="main4"><div class="main5"><div class="main6">
          <div class="main7"><div class="main8"><div class="main9">
            <p>text</p>
          </div></div></div></div></div></div></div></div></div>
        </body></html>"#;
        let document = scraper::Html::parse_document(html);
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".main1");

        assert!(suggestions.len() <= 5);
    }

    #[test]
    fn test_default_inspector_suggest_empty_html() {
        let document = scraper::Html::parse_document("");
        let inspector = DefaultDomInspector::new();
        let suggestions = inspector.suggest(&document, ".anything");

        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_default_inspector_builder_methods() {
        let inspector = DefaultDomInspector::new()
            .with_node_cap(500)
            .with_threshold(0.8);

        // Verify via behavior: with a cap of 500, a 600-element DOM truncates
        let html = "<html><body>".to_owned()
            + &"<div>".repeat(600)
            + &"</div>".repeat(600)
            + "</body></html>";
        let document = scraper::Html::parse_document(&html);
        let report = inspector.inspect(&document);
        assert!(report.truncated);
        assert_eq!(report.element_count, 500);
    }
}
