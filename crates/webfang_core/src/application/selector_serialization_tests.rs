use super::*;

const BASE: &str = "https://example.com/docs/page.html";

/// Pure serialization: what the fragment looks like before any converter.
struct HtmlCase {
    name: &'static str,
    html: &'static str,
    selector: &'static str,
    /// `None` means the selector matched nothing.
    expected: Option<&'static str>,
}

/// Serialize `html` through `selector`, resolved against `BASE`.
///
/// Both case tables below share this arrange prefix (jscpd counted it twice
/// before it moved here).
fn serialize_case(html: &str, selector: &str) -> Option<String> {
    let document = scraper::Html::parse_document(html);
    let css =
        scraper::Selector::parse(selector).expect("every table selector is a valid CSS selector");
    let base = url::Url::parse(BASE).expect("BASE is a valid absolute URL");
    serialize_matches(&document, &css, &base)
}

#[test]
fn selector_fragment_html_table() {
    let cases = [
        HtmlCase {
            name: "nested_div_emitted_once",
            html: r#"<div class="outer"><div class="inner">x</div></div>"#,
            selector: "div",
            expected: Some(r#"<div class="outer"><div class="inner">x</div></div>"#),
        },
        HtmlCase {
            name: "relative_href_and_img_src",
            html: r#"<a href="/root">a</a><img src="img.png">"#,
            selector: "a, img",
            expected: Some(
                "<a href=\"https://example.com/root\">a</a>\n\n\
                 <img src=\"https://example.com/docs/img.png\">",
            ),
        },
        HtmlCase {
            name: "unresolvable_schemes_untouched",
            html: r##"<a href="mailto:a@b.co">m</a><a href="#top">t</a><a href="javascript:void(0)">j</a>"##,
            selector: "a",
            expected: Some(
                "<a href=\"mailto:a@b.co\">m</a>\n\n\
                 <a href=\"#top\">t</a>\n\n\
                 <a href=\"javascript:void(0)\">j</a>",
            ),
        },
        HtmlCase {
            name: "two_siblings_joined_by_blank_line",
            html: "<p>one</p><p>two</p>",
            selector: "p",
            expected: Some("<p>one</p>\n\n<p>two</p>"),
        },
        HtmlCase {
            name: "no_match_is_none",
            html: "<p>x</p>",
            selector: "blockquote",
            expected: None,
        },
    ];

    let mut failures = Vec::new();
    for case in cases {
        let actual = serialize_case(case.html, case.selector);
        if actual.as_deref() != case.expected {
            failures.push(format!(
                "case `{}`\n  expected: {:?}\n  actual:   {:?}",
                case.name, case.expected, actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} selector case(s) diverged:\n{}",
        failures.len(),
        failures.join("\n---\n")
    );
}

/// What the user receives in the `.md`: fragment → the production converter.
struct MdCase {
    name: &'static str,
    html: &'static str,
    selector: &'static str,
    expected: &'static str,
}

#[test]
fn selector_fragment_markdown_table() {
    let cases = [
        MdCase {
            name: "h3",
            html: "<h3>Section</h3>",
            selector: "h3",
            expected: "### Section",
        },
        MdCase {
            name: "table",
            html: "<table><thead><tr><th>Name</th><th>Value</th></tr></thead>\
                   <tbody><tr><td>Foo</td><td>42</td></tr></tbody></table>",
            selector: "table",
            // A real GFM table: the whole point of bypassing Readability.
            expected: "| Name | Value |\n| ---- | ----- |\n| Foo  | 42    |",
        },
        MdCase {
            name: "characterize_li_orphan_bullet",
            html: "<li>one</li><li>two</li>",
            selector: "li",
            // Characterization: htmd falls back to a bullet when the parent
            // is not <ol>, so an orphan <li> still renders as a list item.
            expected: "*   one\n*   two",
        },
        MdCase {
            name: "characterize_tr_inside_table",
            html: "<table><tbody><tr><td>a</td><td>b</td></tr></tbody></table>",
            selector: "tr",
            // #2540: the synthetic <table><tbody> keeps the cell boundary,
            // and the two-column header synthesis turns the pair into a
            // real GFM table instead of two paragraphs.
            expected: "| Column 1 | Column 2 |\n| -------- | -------- |\n| a        | b        |",
        },
        MdCase {
            name: "characterize_two_tr_in_one_tbody",
            html: "<table><tbody><tr><td>a</td><td>b</td></tr>\
                   <tr><td>c</td><td>d</td></tr></tbody></table>",
            selector: "tr",
            // #2540: every matched row lands in ONE synthetic table and the
            // header is synthesized once, so both rows stay in one grid.
            // Multi-row is the common case, so it is pinned explicitly.
            expected: "| Column 1 | Column 2 |\n| -------- | -------- |\n| a        | b        |\n| c        | d        |",
        },
        MdCase {
            name: "characterize_td_inside_table",
            html: "<table><tbody><tr><td>a</td></tr></tbody></table>",
            selector: "td",
            // Characterization: a lone cell has no row to belong to, and
            // #2540 deliberately does not synthesize a header for a single
            // column — there is no grid to preserve.
            expected: "a",
        },
        MdCase {
            name: "characterize_strong_inline_separator",
            html: "<strong>a</strong><strong>b</strong>",
            selector: "strong",
            // Characterization: HTML collapses the blank line between two
            // inline matches into a space, so both end up in one paragraph.
            expected: "**a** **b**",
        },
    ];

    let mut failures = Vec::new();
    for case in cases {
        let Some(fragment) = serialize_case(case.html, case.selector) else {
            failures.push(format!(
                "case `{}` matched nothing — fixture is wrong, not the code",
                case.name
            ));
            continue;
        };
        let markdown = fallback::extract_text(&fragment);
        if markdown.trim() != case.expected {
            failures.push(format!(
                "case `{}`\n  fragment: {fragment}\n  markdown:\n{markdown}",
                case.name
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} markdown case(s) diverged:\n{}",
        failures.len(),
        failures.join("\n---\n")
    );
}
