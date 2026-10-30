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
         meta[itemprop=\"datePublished\"], \
         meta[name=\"parsely-pub-date\"]",
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

/// JSON-LD types that carry a publication date.
///
/// The gate exists for **precision**, not for parity. A page's JSON-LD
/// routinely describes more than the article: a `WebSite`, a `Comment`, a
/// `Review`, a related `Article`. Without a type check the first
/// `datePublished` in the document wins, and a comment's timestamp becomes the
/// page's publication date.
///
/// The list and the surrounding logic (`@context` required, `@graph` consulted
/// only when `@type` is absent, an array contributing its first article-typed
/// element) are transcribed from **legible 0.5.1** (`src/metadata.rs`,
/// `src/constants.rs`). It is private there and it can drift: fixture parity
/// would not notice a type legible adds, because no fixture uses it. When
/// bumping legible, diff this list against its `JSON_LD_ARTICLE_TYPES`.
///
/// legible 0.4.3 had the same 19 names but as a regex
/// (`^Article|AdvertiserContentArticle|…|APIReference$`) where only the first
/// and last alternatives are anchored, so it matched the middle ones as
/// substrings — `"ArticleComment"` passed there and fails here. That is one
/// more reason the list is a list and not a regex.
///
/// Accepted cost: a block without a schema.org `@context`, or with a type
/// outside this list (`Recipe`, `Review`, `WebPage`), is ignored and the
/// cascade falls through to `<meta>`. Rare for article pages, and falling
/// through is what the full-page route does too.
const JSON_LD_ARTICLE_TYPES: &[&str] = &[
    "APIReference",
    "AdvertiserContentArticle",
    "AnalysisNewsArticle",
    "Article",
    "AskPublicNewsArticle",
    "BackgroundNewsArticle",
    "BlogPosting",
    "DiscussionForumPosting",
    "LiveBlogPosting",
    "MedicalScholarlyArticle",
    "NewsArticle",
    "OpinionNewsArticle",
    "Report",
    "ReportageNewsArticle",
    "ReviewNewsArticle",
    "SatiricalArticle",
    "ScholarlyArticle",
    "SocialMediaPosting",
    "TechArticle",
];

fn is_article_type(s: &str) -> bool {
    let name = s
        .strip_prefix("https://schema.org/")
        .or_else(|| s.strip_prefix("http://schema.org/"))
        .or_else(|| {
            let (prefix, name) = s.split_once(':')?;
            (!prefix.is_empty() && !name.is_empty()).then_some(name)
        })
        .unwrap_or(s);
    JSON_LD_ARTICLE_TYPES.contains(&name)
}

fn is_schema_org_url(s: &str) -> bool {
    matches!(
        s,
        "https://schema.org" | "http://schema.org" | "https://schema.org/" | "http://schema.org/"
    )
}

fn is_schema_org_context(value: &Value) -> bool {
    match value {
        Value::String(s) => is_schema_org_url(s),
        Value::Object(map) => map
            .get("@vocab")
            .and_then(Value::as_str)
            .is_some_and(is_schema_org_url),
        _ => false,
    }
}

fn typed_article(value: &Value) -> Option<&Value> {
    value
        .get("@type")
        .and_then(Value::as_str)
        .is_some_and(is_article_type)
        .then_some(value)
}

/// The article node inside a JSON-LD payload, following `@graph`.
///
/// An array contributes its first article-typed element, the payload must
/// declare a schema.org `@context`, and a payload with no `@type` is resolved
/// through `@graph`. Anything else is not an article and is skipped, so a
/// `WebSite` block — or a comment's — cannot hijack the date. See
/// [`JSON_LD_ARTICLE_TYPES`] for why the gate exists.
fn article_node(parsed: &Value) -> Option<&Value> {
    let candidate = match parsed {
        Value::Array(items) => items.iter().find(|item| typed_article(item).is_some())?,
        Value::Object(_) => parsed,
        _ => return None,
    };
    let map = candidate.as_object()?;
    if !map.get("@context").is_some_and(is_schema_org_context) {
        return None;
    }
    if let Some(node) = typed_article(candidate) {
        return Some(node);
    }
    map.get("@graph")?
        .as_array()?
        .iter()
        .find(|item| typed_article(item).is_some())
}

/// Pull `datePublished` out of an article node. `dateModified` is deliberately
/// ignored: it is a different semantic, and using it would publish an update
/// timestamp as the publication date.
fn date_from_json(parsed: &Value) -> Option<String> {
    article_node(parsed)?
        .get("datePublished")
        .and_then(Value::as_str)
        .and_then(non_empty)
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


    /// The cascade and legible's metadata pass are two independent
    /// implementations. On the same page they must agree wherever legible has a
    /// source, or the same site shows one date with `--selector` and another
    /// without it.
    ///
    /// Deliberately NOT a parity assertion for every source: this cascade is
    /// broader by design (`og:published_time`, `name="date"`, microdata,
    /// `<article><time>`), so it can find a date legible misses. Finding more
    /// is not the divergence that matters; disagreeing on the same source is.
    #[test]
    fn date_cascade_agrees_with_legible_on_shared_sources() {
        let shared = [
            r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-03-04T10:00:00Z"}</script></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><meta property="article:published_time" content="2026-02-02T08:30:00Z"></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><meta name="parsely-pub-date" content="2026-02-06"></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"WebSite"},{"@type":"Article","datePublished":"2026-05-01"}]}</script></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2025-12-31T23:59:59Z"></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><script type="application/ld+json">{"@type":"Article","datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2026-08-14"></head><body><article><p>x</p></article></body></html>"#,
            r#"<html><head><script type="application/ld+json">{"@graph":[{"@type":"Article","datePublished":"2026-01-01"}]}</script><meta property="article:published_time" content="2026-08-15"></head><body><article><p>x</p></article></body></html>"#,
        ];

        let mut failures = Vec::new();
        for html in shared {
            let ours = extract_date(html);
            let legible = crate::domain::html_extraction::readability::parse(
                html,
                Some("https://example.com/post"),
            )
            .ok()
            .and_then(|article| article.published_time);
            if ours != legible {
                failures.push(format!(
                    "html: {html}\n  ours:    {ours:?}\n  legible: {legible:?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} fixture(s) disagree with legible:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }

    #[test]
    fn date_extraction_cascade_table() {
        let cases = [
            Case {
                name: "jsonld_date_published",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-03-04T10:00:00Z"}</script></head><body></body></html>"#,
                expected: Some("2026-03-04T10:00:00Z"),
            },
            Case {
                name: "jsonld_graph_array",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"WebSite"},{"@type":"Article","datePublished":"2026-05-01"}]}</script></head><body></body></html>"#,
                expected: Some("2026-05-01"),
            },
            Case {
                name: "jsonld_beats_meta",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2025-12-31T23:59:59Z"></head><body></body></html>"#,
                expected: Some("2026-01-01"),
            },
            // The JSON-LD gate is deliberately identical to legible's: without a
            // schema.org @context the block is not trusted, so both routes ignore
            // it and both fall through to the same meta tag.
            Case {
                name: "jsonld_without_context_is_ignored",
                html: r#"<html><head><script type="application/ld+json">{"datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2026-08-11"></head><body></body></html>"#,
                expected: Some("2026-08-11"),
            },
            Case {
                name: "jsonld_without_context_is_ignored_even_when_article_typed",
                html: r#"<html><head><script type="application/ld+json">{"@type":"Article","datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2026-08-14"></head><body></body></html>"#,
                expected: Some("2026-08-14"),
            },
            Case {
                name: "jsonld_graph_without_context_is_ignored",
                html: r#"<html><head><script type="application/ld+json">{"@graph":[{"@type":"Article","datePublished":"2026-01-01"}]}</script><meta property="article:published_time" content="2026-08-15"></head><body></body></html>"#,
                expected: Some("2026-08-15"),
            },
            Case {
                name: "jsonld_non_article_type_is_ignored",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"WebPage","datePublished":"2026-01-01"}</script><meta property="article:published_time" content="2026-08-12"></head><body></body></html>"#,
                expected: Some("2026-08-12"),
            },
            Case {
                name: "jsonld_graph_without_article_type_is_ignored",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"WebSite","datePublished":"2026-01-01"}]}</script><meta property="article:published_time" content="2026-08-13"></head><body></body></html>"#,
                expected: Some("2026-08-13"),
            },
            Case {
                name: "jsonld_absolute_type_url_is_recognized",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"https://schema.org/NewsArticle","datePublished":"2026-11-14"}</script></head><body></body></html>"#,
                expected: Some("2026-11-14"),
            },
            Case {
                name: "jsonld_context_object_with_vocab_is_recognized",
                html: r#"<html><head><script type="application/ld+json">{"@context":{"@vocab":"https://schema.org"},"@type":"BlogPosting","datePublished":"2026-11-15"}</script></head><body></body></html>"#,
                expected: Some("2026-11-15"),
            },
            Case {
                name: "jsonld_array_payload_uses_article_element",
                html: r#"<html><head><script type="application/ld+json">[{"@type":"WebSite"},{"@context":"https://schema.org","@type":"Article","datePublished":"2026-11-16"}]</script></head><body></body></html>"#,
                expected: Some("2026-11-16"),
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
                name: "malformed_jsonld_falls_through_to_meta",
                html: r#"<html><head><script type="application/ld+json">{not json</script><meta property="article:published_time" content="2026-08-09"></head><body></body></html>"#,
                expected: Some("2026-08-09"),
            },
            Case {
                name: "jsonld_without_date_published_falls_through",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"No date here"}</script><meta property="article:published_time" content="2026-08-10"></head><body></body></html>"#,
                expected: Some("2026-08-10"),
            },
            Case {
                name: "date_modified_is_not_a_publish_date",
                html: r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","dateModified":"2026-09-11"}</script></head><body><article><p>edited later</p></article></body></html>"#,
                // A different semantic: falling back to it would publish an
                // update timestamp as if it were the publication date.
                expected: None,
            },
            Case {
                name: "multiple_jsonld_blocks_second_one_wins",
                html: r#"<html><head><script type="application/ld+json">{"@type":"WebSite"}</script><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-10-13"}</script></head><body></body></html>"#,
                expected: Some("2026-10-13"),
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

    /// Places where this cascade is intentionally more permissive than legible.
    ///
    /// These are not bugs to fix: legible has no source at all for them, so
    /// "finding more" cannot make the two routes disagree — there is nothing on
    /// the other side to disagree with. Pinning them here means a future change
    /// that silently narrows the cascade fails a test instead of quietly
    /// emptying the frontmatter.
    #[test]
    fn documented_divergences_from_legible() {
        let cases = [
            // A JSON-LD block that passes legible's gate but carries no
            // datePublished stops legible's loop (break) before it ever sees the
            // second block. We keep looking.
            (
                "later_jsonld_block_after_a_dateless_article",
                r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"no date"}</script><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","datePublished":"2026-12-01"}</script></head><body></body></html>"#,
                Some("2026-12-01"),
            ),
            // legible's meta pass only picks article:published_time and
            // parsely-pub-date; these three sources are ours alone.
            (
                "og_published_time",
                r#"<html><head><meta property="og:published_time" content="2026-12-02"></head><body></body></html>"#,
                Some("2026-12-02"),
            ),
            (
                "name_date_meta",
                r#"<html><head><meta name="date" content="2026-12-03"></head><body></body></html>"#,
                Some("2026-12-03"),
            ),
            (
                "article_time",
                r#"<html><body><article><time datetime="2026-12-04">Dec 4</time></article></body></html>"#,
                Some("2026-12-04"),
            ),
        ];

        let mut failures = Vec::new();
        for (name, html, expected) in cases {
            let ours = extract_date(html);
            let legible = crate::domain::html_extraction::readability::parse(
                html,
                Some("https://example.com/post"),
            )
            .ok()
            .and_then(|article| article.published_time);
            if ours.as_deref() != expected {
                failures.push(format!(
                    "case `{name}`: ours = {ours:?}, expected {expected:?}"
                ));
            }
            if legible.is_some() {
                failures.push(format!(
                    "case `{name}`: legible now finds {legible:?} too — re-check whether this is still a divergence"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} divergence case(s) drifted:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }
}
