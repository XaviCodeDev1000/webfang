//! Multi-strategy publication-date extraction
//!
//! The `--selector` path never runs Readability, and `published_time` used to
//! come from it. This cascade replaces that source, ordered from most to least
//! reliable — the same shape as [`author_extractor`](super::author_extractor):
//!
//! 1. JSON-LD structured data (`datePublished`)
//! 2. `<meta>` tags (`article:published_time`, `og:published_time`,
//!    `name="date"`, `itemprop="datePublished"`)
//! 3. Microdata (`[itemprop="datePublished"]`)
//! 4. `<time datetime>` inside an `<article>` container
//!
//! The output is the source string, trimmed — not a re-formatted date. That is
//! exactly what legible returns for `published_time`, so the frontmatter never
//! carries two date styles depending on which route produced the page.
//!
//! # Rules Applied
//!
//! - **own-borrow-over-clone**: Accept `&str`, return owned only at the boundary
//! - **err-no-unwrap-prod**: Static selectors use `expect("BUG: ...")` only for
//!   compile-time-known-valid selectors
//! - **opt-inline**: CSS selectors compiled once via `LazyLock`

use std::sync::LazyLock;

use scraper::{Html, Selector};
use serde_json::Value;

// All selectors below are compile-time-constant strings, so `Selector::parse`
// cannot fail at runtime; each `expect` documents a true invariant.
#[allow(clippy::expect_used)]
static SEL_JSONLD: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("script[type=\"application/ld+json\"]")
        // LCOV_EXCL_LINE defensive: compile-time-selector — constant selector string, parse cannot fail
        .expect("BUG: invalid CSS selector application/ld+json")
});
#[allow(clippy::expect_used)]
static SEL_META_DATE: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "meta[property=\"article:published_time\"], \
         meta[property=\"og:published_time\"], \
         meta[name=\"date\"], \
         meta[itemprop=\"datePublished\"]",
    )
    // LCOV_EXCL_LINE defensive: compile-time-selector — constant selector string, parse cannot fail
    .expect("BUG: invalid CSS selector meta date")
});
#[allow(clippy::expect_used)]
static SEL_ITEMPROP_DATE: LazyLock<Selector> = LazyLock::new(|| {
    // LCOV_EXCL_LINE defensive: compile-time-selector — constant selector string, parse cannot fail
    Selector::parse("[itemprop=\"datePublished\"]")
        .expect("BUG: invalid CSS selector itemprop=datePublished")
});
/// Deliberately anchored to `article`: the first bare `<time>` on a page is
/// usually a comment timestamp or a "posted 3 hours ago" label, not the
/// publication date.
#[allow(clippy::expect_used)]
static SEL_ARTICLE_TIME: LazyLock<Selector> = LazyLock::new(|| {
    // LCOV_EXCL_LINE defensive: compile-time-selector — constant selector string, parse cannot fail
    Selector::parse("article time[datetime]").expect("BUG: invalid CSS selector article time")
});

/// One date-extraction strategy.
pub trait DateExtractor: Send + Sync {
    /// Human-readable strategy name for logging.
    fn name(&self) -> &str;

    /// Try to extract a publication date from the parsed document.
    fn extract(&self, document: &Html) -> Option<String>;
}

/// Strategy 1: JSON-LD structured data.
#[derive(Default)]
pub struct JsonLdDateExtractor;

impl DateExtractor for JsonLdDateExtractor {
    fn name(&self) -> &str {
        "json-ld"
    }

    fn extract(&self, document: &Html) -> Option<String> {
        document.select(&SEL_JSONLD).find_map(|script| {
            let raw: String = script.text().collect();
            let value: Value = serde_json::from_str(&raw).ok()?;
            date_from_json(&value)
        })
    }
}

/// Strategy 2: `<meta>` tags.
#[derive(Default)]
pub struct MetaDateExtractor;

impl DateExtractor for MetaDateExtractor {
    fn name(&self) -> &str {
        "meta"
    }

    fn extract(&self, document: &Html) -> Option<String> {
        document
            .select(&SEL_META_DATE)
            .find_map(|element| element.value().attr("content").and_then(non_empty))
    }
}

/// Strategy 3: microdata.
#[derive(Default)]
pub struct ItempropDateExtractor;

impl DateExtractor for ItempropDateExtractor {
    fn name(&self) -> &str {
        "itemprop"
    }

    fn extract(&self, document: &Html) -> Option<String> {
        document.select(&SEL_ITEMPROP_DATE).find_map(|element| {
            element
                .value()
                .attr("content")
                .or_else(|| element.value().attr("datetime"))
                .and_then(non_empty)
                .or_else(|| text_content(&element))
        })
    }
}

/// Strategy 4: `<time datetime>` inside an article container.
#[derive(Default)]
pub struct ArticleTimeExtractor;

impl DateExtractor for ArticleTimeExtractor {
    fn name(&self) -> &str {
        "article-time"
    }

    fn extract(&self, document: &Html) -> Option<String> {
        document
            .select(&SEL_ARTICLE_TIME)
            .find_map(|element| element.value().attr("datetime").and_then(non_empty))
    }
}

/// Try each strategy in order, returning the first non-empty date.
///
/// Convenience wrapper that parses `html` once. Hot-path callers that already
/// hold a parsed DOM should call [`extract_date_from_document`] directly.
///
/// The input must be the **raw** page HTML: `clean_html` strips `<script>`, so
/// the JSON-LD strategy finds nothing on a cleaned document.
#[must_use]
pub fn extract_date(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    extract_date_from_document(&document)
}

/// Try each strategy in order against an already-parsed **raw** document.
#[must_use]
pub fn extract_date_from_document(document: &Html) -> Option<String> {
    let strategies: [&dyn DateExtractor; 4] = [
        &JsonLdDateExtractor,
        &MetaDateExtractor,
        &ItempropDateExtractor,
        &ArticleTimeExtractor,
    ];
    strategies
        .iter()
        .find_map(|strategy| strategy.extract(document))
}

/// Pull `datePublished` out of a JSON-LD payload, following `@graph`.
fn date_from_json(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().find_map(date_from_json),
        Value::Object(map) => map
            .get("datePublished")
            .and_then(Value::as_str)
            .and_then(non_empty)
            .or_else(|| map.get("@graph").and_then(date_from_json)),
        _ => None,
    }
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn text_content(element: &scraper::ElementRef<'_>) -> Option<String> {
    let collapsed: String = element.text().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    non_empty(&collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Case {
        name: &'static str,
        html: &'static str,
        expected: Option<&'static str>,
    }

    #[test]
    fn date_extraction_cascade_table() {
        let cases = [
            Case {
                name: "jsonld_date_published",
                html: r#"<html><head><script type="application/ld+json">{"datePublished":"2026-03-04T10:00:00Z"}</script></head><body></body></html>"#,
                expected: Some("2026-03-04T10:00:00Z"),
            },
            Case {
                name: "jsonld_graph_array",
                html: r#"<html><head><script type="application/ld+json">{"@graph":[{"@type":"WebSite"},{"datePublished":"2026-05-01"}]}</script></head><body></body></html>"#,
                expected: Some("2026-05-01"),
            },
            Case {
                name: "jsonld_beats_meta",
                html: r#"<html><head><script type="application/ld+json">{"datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2025-12-31T23:59:59Z"></head><body></body></html>"#,
                expected: Some("2026-01-01"),
            },
            Case {
                name: "meta_article_published_time",
                html: r#"<html><head><meta property="article:published_time" content="2026-02-02T08:30:00Z"></head><body></body></html>"#,
                expected: Some("2026-02-02T08:30:00Z"),
            },
            Case {
                name: "meta_og_published_time",
                html: r#"<html><head><meta property="og:published_time" content="2026-02-03"></head><body></body></html>"#,
                expected: Some("2026-02-03"),
            },
            Case {
                name: "itemprop_content_attribute",
                html: r#"<html><body><span itemprop="datePublished" content="2026-04-05">5 April</span></body></html>"#,
                expected: Some("2026-04-05"),
            },
            Case {
                name: "article_time_datetime",
                html: r#"<html><body><article><time datetime="2026-06-07T12:00:00Z">June 7</time></article></body></html>"#,
                expected: Some("2026-06-07T12:00:00Z"),
            },
            Case {
                name: "stray_time_outside_article_is_ignored",
                html: r#"<html><body><time datetime="2026-06-01">comment</time><div class="post"><time datetime="2026-06-02">post</time></div></body></html>"#,
                expected: None,
            },
            Case {
                name: "nothing_found_is_none",
                html: "<html><body><p>no metadata here</p></body></html>",
                expected: None,
            },
            Case {
                name: "blank_content_falls_through_to_next_meta",
                html: r#"<html><head><meta property="article:published_time" content="   "><meta name="date" content="2026-07-08"></head><body></body></html>"#,
                expected: Some("2026-07-08"),
            },
        ];

        let mut failures = Vec::new();
        for case in cases {
            let actual = extract_date(case.html);
            if actual.as_deref() != case.expected {
                failures.push(format!(
                    "case `{}`\n  expected: {:?}\n  actual:   {:?}",
                    case.name, case.expected, actual
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} date case(s) diverged:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }
}
