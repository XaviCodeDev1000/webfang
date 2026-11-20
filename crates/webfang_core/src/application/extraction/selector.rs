//! Pure CSS-selector extraction: match a selector against a cleaned DOM and
//! serialize the matches (table-aware), with the diagnostic path for an
//! invalid selector and the no-match fallback.

use crate::application::diagnostic::build_diagnostic;
use crate::domain::{DomInspectorPort, ExtractResult, SelectorErrorKind};
use std::collections::HashSet;
use tracing::{debug, warn};

/// Extract HTML content using a CSS selector.
///
/// When `selector` is not "body", parses the HTML and extracts all elements
/// matching the selector. Returns the outer HTML of matched elements wrapped
/// in a `<div>` for Readability processing. If no elements match or the
/// selector is invalid, returns [`ExtractResult::Fallback`] with the full
/// HTML and an optional diagnostic (when an inspector is provided).
///
/// # Arguments
/// * `html` - The HTML document to extract from
/// * `selector` - CSS selector string (use `"body"` to skip extraction)
/// * `base_url` - Page URL used to resolve relative links in the fragment
/// * `inspector` - Optional DOM inspector for diagnostics on failure paths
pub fn extract_with_selector(
    html: &str,
    selector: &str,
    base_url: &url::Url,
    inspector: Option<&dyn DomInspectorPort>,
) -> ExtractResult {
    if selector == "body" {
        return ExtractResult::Matched(html.to_owned());
    }

    // Early check: empty or whitespace-only HTML. `scraper::Html::parse_document("")`
    // creates 3 implicit elements (html, head, body), so without this check the
    // selector matching would fall through to ZeroMatches instead of
    // EmptyDocument — leaving SelectorErrorKind::EmptyDocument as dead code.
    if html.trim().is_empty() {
        let document = scraper::Html::parse_document(html);
        return fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::EmptyDocument,
            selector,
            "HTML document is empty or whitespace-only, falling back with EmptyDocument diagnostic",
        );
    }

    let document = scraper::Html::parse_document(html);
    let sel = match scraper::Selector::parse(selector) {
        Ok(s) => s,
        Err(e) => {
            return fallback(
                html,
                inspector,
                &document,
                // The diagnostic keeps the raw crate error for debugging;
                // the user-facing WARN must not leak library jargon (#761).
                SelectorErrorKind::InvalidSelector(e.to_string()),
                selector,
                &invalid_selector_message(selector),
            );
        },
    };

    match serialize_matches(&document, &sel, base_url) {
        Some(fragment) => {
            debug!(
                "CSS selector '{}' produced a {}-byte fragment",
                selector,
                fragment.len()
            );
            ExtractResult::Matched(fragment)
        },
        None => fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::ZeroMatches,
            selector,
            &format!("CSS selector '{selector}' matched 0 elements, falling back to full HTML"),
        ),
    }
}

/// Whether the caller must use the fragment as-is instead of running Readability.
///
/// The default `body` selector returns `Matched` too — it means "no extraction
/// happened, the whole document is the input" — so `is_matched()` alone would
/// skip Readability for every page. Both extraction routes ask this question,
/// so the rule lives in one place (#2534).
#[must_use]
pub(crate) fn selector_supplied_fragment(selector: &str, result: &ExtractResult) -> bool {
    selector != "body" && result.is_matched()
}

/// Serialize the elements matched by `selector` into one fragment.
///
/// Matches nested inside another match are dropped so each element is emitted
/// once, and `href`/`src` are resolved against `base_url` — the two jobs
/// Readability used to do implicitly for this path. Blocks are separated by a
/// blank line, which is what makes `--selector h3` yield two paragraphs instead
/// of two concatenated headings.
///
/// Returns `None` when the selector matches nothing.
pub(super) fn serialize_matches(
    document: &scraper::Html,
    selector: &scraper::Selector,
    base_url: &url::Url,
) -> Option<String> {
    let matches: Vec<scraper::ElementRef<'_>> = document.select(selector).collect();
    if matches.is_empty() {
        return None;
    }

    // `ancestors()` starts at the parent, so a match never discards itself.
    let selected: HashSet<_> = matches.iter().map(|element| element.id()).collect();
    let blocks: Vec<String> = matches
        .iter()
        .filter(|element| {
            !element
                .ancestors()
                .any(|ancestor| selected.contains(&ancestor.id()))
        })
        .map(scraper::ElementRef::html)
        .collect();

    let fragment = blocks.join("\n\n");
    // `htmd::convert` parses with `parse_document`, so a row or cell fragment
    // loses its tags at conversion time: the HTML5 parser drops `<tr>`/`<td>`
    // outside a table context and the cells collapse into bare text. Restoring
    // the synthetic ancestor here is the only place it can be restored — the
    // converter cannot recover context the fragment never had.
    let fragment = if matches
        .first()
        .is_some_and(|element| needs_table_context(element.value().name()))
    {
        format!("<table><tbody>{fragment}</tbody></table>")
    } else {
        fragment
    };

    // #2540: prepare the fragment ONCE, so the stored `html` and the Markdown
    // derived from it can never disagree about a table's shape.
    let fragment = crate::application::fragment_markdown::with_header(&fragment);
    Some(crate::application::fragment_urls::absolutize_urls(
        &fragment, base_url,
    ))
}

/// Whether a matched tag only means something inside a `<table>`.
///
/// `caption` is deliberately absent: it belongs outside `<tbody>`, so the
/// synthetic wrapper below would be wrong for it.
pub(super) fn needs_table_context(tag: &str) -> bool {
    matches!(tag, "tr" | "td" | "th" | "thead" | "tbody" | "tfoot")
}

/// Emit the fallback warning and build a [`ExtractResult::Fallback`] carrying
/// the full HTML and a diagnostic for the given [`SelectorErrorKind`], when an
/// inspector is available.
/// Clean user-facing message for an invalid CSS selector (#761).
///
/// The `selectors` crate's error text leaks library jargon
/// (`NoQualifiedNameInAttributeSelector(...)`, "Please report this to the
/// developer") into production stderr. The raw error is preserved in the
/// [`SelectorErrorKind::InvalidSelector`] diagnostic for debugging; only this
/// sanitized message reaches the user.
pub(super) fn invalid_selector_message(selector: &str) -> String {
    format!("selector CSS inválido: '{selector}' — se usó el HTML completo como fallback")
}

pub(super) fn fallback(
    html: &str,
    inspector: Option<&dyn DomInspectorPort>,
    document: &scraper::Html,
    kind: SelectorErrorKind,
    selector: &str,
    message: &str,
) -> ExtractResult {
    warn!("{}", message);
    ExtractResult::Fallback {
        html: html.to_owned(),
        diagnostic: build_diagnostic(inspector, document, kind, selector),
    }
}
