//! Synthetic table headers for a matched selector fragment (#2540).
//!
//! `htmd` builds a GFM table only when it can infer a header row, and it infers
//! that row from `<th>` cells — so a **headerless** data table degrades to one
//! paragraph per cell and the row/column relationship is lost:
//!
//! ```text
//! <table><tr><td>a</td><td>b</td></tr></table>   ->   "a\n\nb"
//! ```
//!
//! #2540 chose to synthesize a header row so the grid survives, and to do it
//! **only for two or more columns**: a one-column table carries no row/column
//! relationship to preserve, and turning `--selector td` into a table would be
//! a surprise with nothing gained.

/// Prepend a synthetic `<thead>` to every headerless table that has at least
/// two columns.
///
/// Two passes by necessity: counting columns needs a real DOM (`scraper`),
/// while inserting into the serialized HTML needs the rewriter (`lol_html`),
/// which has no child queries. Tables are matched between the passes by
/// document order, which both parsers agree on.
///
/// Insertion respects the table model order: when a table leads with
/// `<caption>` and/or `<colgroup>`, the synthetic `<thead>` goes after them
/// instead of as the first child. A table that already has a `<thead>` —
/// even with `<td>` cells — or any `<th>` is left alone: it already has a
/// header row.
///
/// The header width is the grid width (`colspan`/`rowspan` aware), capped
/// at [`MAX_GRID_COLUMNS`]: wider tables are left alone rather than
/// synthesizing a header that would not match the grid.
///
/// If the rewrite fails, the fragment is returned unchanged and the failure
/// is logged: the fallback keeps extraction alive, but the synthesis is then
/// silently absent from the output, so the degradation must be observable.
pub(crate) fn with_header(fragment: &str) -> String {
    if !fragment.to_ascii_lowercase().contains("<table") {
        return fragment.to_owned();
    }

    let document = scraper::Html::parse_fragment(fragment);
    let headers: Vec<Option<TableHeader>> = document
        .select(&selector("table"))
        .map(|table| header_for_table(&table))
        .collect();
    if headers.iter().all(Option::is_none) {
        return fragment.to_owned();
    }

    use lol_html::html_content::ContentType;
    let position = std::cell::Cell::new(0usize);
    let current = std::cell::Cell::new(None::<usize>);
    let emitted_for = std::cell::Cell::new(None::<usize>);
    let settings = lol_html::RewriteStrSettings {
        element_content_handlers: vec![
            lol_html::element!("table", |el| {
                let index = position.get();
                position.set(index + 1);
                current.set(Some(index));
                if let Some(Some(header)) = headers.get(index) {
                    if header.after.is_none() {
                        el.prepend(&header.thead, ContentType::Html);
                    }
                }
                Ok(())
            }),
            lol_html::element!("caption", |el| {
                if let Some(index) = current.get() {
                    if let Some(Some(header)) = headers.get(index) {
                        if matches!(header.after, Some(Anchor::Caption))
                            && emitted_for.get() != Some(index)
                        {
                            el.after(&header.thead, ContentType::Html);
                            emitted_for.set(Some(index));
                        }
                    }
                }
                Ok(())
            }),
            lol_html::element!("colgroup", |el| {
                if let Some(index) = current.get() {
                    if let Some(Some(header)) = headers.get(index) {
                        if matches!(header.after, Some(Anchor::Colgroup))
                            && emitted_for.get() != Some(index)
                        {
                            el.after(&header.thead, ContentType::Html);
                            emitted_for.set(Some(index));
                        }
                    }
                }
                Ok(())
            }),
        ],
        ..lol_html::RewriteStrSettings::default()
    };
    match lol_html::rewrite_str(fragment, settings) {
        Ok(rewritten) => rewritten,
        Err(error) => {
            tracing::warn!(
                error = %error,
                stage = "extract",
                tables = headers.len(),
                fragment_len = fragment.len(),
                "synthetic header insertion failed; returning fragment unchanged"
            );
            fragment.to_owned()
        },
    }
}

/// Where a synthetic `<thead>` goes relative to the table's leading
/// `<caption>`/`<colgroup>` children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
    /// After a leading `<caption>` (no `<colgroup>` follows).
    Caption,
    /// After a leading `<colgroup>` (covers a preceding `<caption>` too).
    Colgroup,
}

/// A synthetic `<thead>` plus the point where it must be inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableHeader {
    thead: String,
    after: Option<Anchor>,
}

/// Widest grid a synthetic header will be built for. Wider (pathological)
/// tables are left alone: a capped header would not match the grid.
const MAX_GRID_COLUMNS: usize = 256;

/// Largest `colspan`/`rowspan` value honoured while sizing the grid;
/// larger values are treated as pathological input, not as widths.
const MAX_SPAN: usize = 64;

/// The `<thead>` a table needs, or `None` when it must be left alone.
fn header_for_table(table: &scraper::ElementRef<'_>) -> Option<TableHeader> {
    // `htmd` already infers the header row when a `<th>` exists anywhere, so
    // there is nothing to synthesize.
    if table.select(&selector("th")).next().is_some() {
        return None;
    }
    // A `<thead>` row is a header row even when its cells are `<td>`: putting
    // `Column N` names in front of it would displace the table's real labels.
    if table.select(&selector("thead")).next().is_some() {
        return None;
    }
    let columns = grid_width(table);
    if !(2..=MAX_GRID_COLUMNS).contains(&columns) {
        return None;
    }
    let cells: String = (1..=columns)
        .map(|index| format!("<th>Column {index}</th>"))
        .collect();
    Some(TableHeader {
        thead: format!("<thead><tr>{cells}</tr></thead>"),
        after: leading_anchor(table),
    })
}

/// Grid width of a table: the widest row extent, `colspan`/`rowspan` aware.
///
/// Rows are simulated left to right; a slot already occupied by a `rowspan`
/// from above is skipped, and each cell claims `colspan` consecutive slots.
/// A missing or unparsable span counts as 1.
fn grid_width(table: &scraper::ElementRef<'_>) -> usize {
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    for row in table.select(&selector("tr")) {
        let r = occupied.len();
        occupied.push(Vec::new());
        let mut col = 0usize;
        for cell in row.children() {
            let Some(element) = cell.value().as_element() else {
                continue;
            };
            if !matches!(element.name(), "td" | "th") {
                continue;
            }
            while col < occupied[r].len() && occupied[r][col] {
                col += 1;
            }
            let colspan = span(element.attr("colspan"));
            let rowspan = span(element.attr("rowspan"));
            for rr in r..(r + rowspan) {
                while occupied.len() <= rr {
                    occupied.push(Vec::new());
                }
                for cc in col..(col + colspan) {
                    if occupied[rr].len() <= cc {
                        occupied[rr].resize(cc + 1, false);
                    }
                    occupied[rr][cc] = true;
                }
            }
            col += colspan;
        }
    }
    occupied.iter().map(Vec::len).max().unwrap_or(0)
}

/// Parse a `colspan`/`rowspan` value, clamped to [`MAX_SPAN`].
fn span(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, MAX_SPAN)
}

/// The anchor for the synthetic `<thead>` given the table's leading element
/// children in document order, or `None` to insert as the first child.
///
/// HTML orders `<caption>` then `<colgroup>` before `<thead>`; inserting
/// after the last of the leading ones keeps the table model order valid.
fn leading_anchor(table: &scraper::ElementRef<'_>) -> Option<Anchor> {
    let mut has_caption = false;
    let mut has_colgroup = false;
    for child in table.children() {
        let Some(element) = child.value().as_element() else {
            continue;
        };
        match element.name() {
            "caption" => has_caption = true,
            "colgroup" => has_colgroup = true,
            _ => break,
        }
    }
    if has_colgroup {
        Some(Anchor::Colgroup)
    } else if has_caption {
        Some(Anchor::Caption)
    } else {
        None
    }
}

/// Parse a selector from a call-site literal.
///
/// The signature accepts any string at runtime; nothing here checks at
/// compile time that it is valid CSS. The contract is a call-site
/// convention: every caller in this module passes a static literal, so an
/// invalid literal panics in tests instead of reaching production — the
/// `expect` below is that panic, kept loud on purpose.
fn selector(css: &str) -> scraper::Selector {
    #[allow(clippy::expect_used)]
    let parsed = scraper::Selector::parse(css)
        // LCOV_EXCL_LINE defensive: call-site convention — every caller passes
        // a static literal, so an invalid literal is a test-time bug, not a
        // runtime input. Keep the panic loud; there is no compile-time check.
        .expect("invariant: the selector literals in this module are valid CSS");
    parsed
}

// Every test of this module drives `with_header`, whose contract is the table
// rewrite: `lol_html::rewrite_str` with the handlers below, where servo_arc
// 0.4.3 raises the Tree-Borrows UB (tracker servo/stylo#210, open upstream).
// Gate the whole module, following the precedent written in
// `domain::html_cleaner::tests`: only a minority of cases return before the
// rewrite (no `<table>`, or an existing `<th>`/`<thead>`), and a textual marker
// cannot tell those from the ones that rewrite — so the module is the unit
// that carries the gate.
//
// With the three production `lol_html` call sites of this crate now declared
// (`domain::html_cleaner`, `application::fragment_urls`,
// `application::fragment_markdown`), the surface this lane sweeps is COMPLETE
// for this crate: no further entry point is hiding behind an earlier abort.
#[cfg(all(test, not(miri)))]
mod tests {
    use super::with_header;

    /// #2540: two columns are enough to preserve a grid, so a header row is
    /// synthesized with one synthetic name per column.
    #[test]
    fn issue_2540_two_columns_get_a_synthetic_header() {
        let out = with_header("<table><tr><td>a</td><td>b</td></tr></table>");
        assert!(
            out.contains("<thead><tr><th>Column 1</th><th>Column 2</th></tr></thead>"),
            "one synthetic name per column, got: {out}"
        );
        assert!(
            out.contains("<tr><td>a</td><td>b</td></tr>"),
            "the data row must survive untouched, got: {out}"
        );
    }

    /// #2540: three columns get three names, not two.
    #[test]
    fn issue_2540_column_count_drives_the_header_width() {
        let out = with_header("<table><tr><td>a</td><td>b</td><td>c</td></tr></table>");
        assert!(
            out.contains("<th>Column 1</th><th>Column 2</th><th>Column 3</th>"),
            "one synthetic name per column, got: {out}"
        );
    }

    /// #2540: a one-column table has no row/column relationship to preserve,
    /// and `--selector td` must not start returning a table.
    #[test]
    fn issue_2540_single_column_is_left_alone() {
        let html = "<table><tr><td>a</td></tr></table>";
        assert_eq!(with_header(html), html);
    }

    /// #2540: a table that already has `<th>` is untouched — `htmd` already
    /// infers its header row, so synthesizing one would duplicate it.
    #[test]
    fn issue_2540_existing_th_is_untouched() {
        let html = "<table><thead><tr><th>A</th><th>B</th></tr></thead>\
                    <tbody><tr><td>1</td><td>2</td></tr></tbody></table>";
        assert_eq!(with_header(html), html);
    }

    /// #2540: a fragment without a table is returned byte-identical, so the
    /// non-table path pays only one `contains` check.
    #[test]
    fn issue_2540_non_table_fragment_is_unchanged() {
        let html = "<p>hello</p><div>world</div>";
        assert_eq!(with_header(html), html);
    }

    /// #2602: with a `<caption>`, the synthetic `<thead>` goes AFTER it —
    /// HTML's table model orders caption first and an inverted order is
    /// invalid markup for the downstream converter.
    #[test]
    fn issue_2602_caption_stays_before_synthetic_header() {
        let out =
            with_header("<table><caption>Tarifas</caption><tr><td>a</td><td>b</td></tr></table>");
        let caption_at = out.find("<caption>").expect("caption must survive");
        let thead_at = out.find("<thead>").expect("header must be synthesized");
        assert!(caption_at < thead_at, "caption must come first, got: {out}");
        assert!(
            out.contains("<tr><td>a</td><td>b</td></tr>"),
            "data row must survive untouched, got: {out}"
        );
    }

    /// #2602: with a `<colgroup>`, the synthetic `<thead>` goes after it,
    /// for the same table-model order reason as `<caption>`.
    #[test]
    fn issue_2602_colgroup_stays_before_synthetic_header() {
        let out = with_header(
            "<table><colgroup><col><col></colgroup><tr><td>a</td><td>b</td></tr></table>",
        );
        let colgroup_at = out.find("<colgroup>").expect("colgroup must survive");
        let thead_at = out.find("<thead>").expect("header must be synthesized");
        assert!(
            colgroup_at < thead_at,
            "colgroup must come first, got: {out}"
        );
    }

    /// #2602: caption and colgroup together keep their relative order, and
    /// the synthetic header lands after both.
    #[test]
    fn issue_2602_caption_and_colgroup_stay_before_header() {
        let out = with_header(
            "<table><caption>Tarifas</caption><colgroup><col><col></colgroup>\
             <tr><td>a</td><td>b</td></tr></table>",
        );
        let caption_at = out.find("<caption>").expect("caption must survive");
        let colgroup_at = out.find("<colgroup>").expect("colgroup must survive");
        let thead_at = out.find("<thead>").expect("header must be synthesized");
        assert!(
            caption_at < colgroup_at && colgroup_at < thead_at,
            "caption < colgroup < thead expected, got: {out}"
        );
    }

    /// #2602: a table whose first row already sits in a `<thead>` with
    /// `<td>` cells has a header row — synthesizing `Column N` in front of
    /// it would displace the table's real labels.
    #[test]
    fn issue_2602_thead_of_td_is_left_alone() {
        let html = "<table><thead><tr><td>A</td><td>B</td></tr></thead>\
                    <tbody><tr><td>1</td><td>2</td></tr></tbody></table>";
        assert_eq!(with_header(html), html);
    }

    /// #2602: with several tables, each header lands on its own table by
    /// document order — a table that already has `<th>` stays untouched
    /// while its neighbours each get a header of their own width.
    #[test]
    fn issue_2602_multi_table_headers_follow_document_order() {
        let out = with_header(
            "<table><tr><td>a</td><td>b</td></tr></table>\
             <table><thead><tr><th>X</th><th>Y</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>\
             <table><tr><td>c</td><td>d</td><td>e</td></tr></table>",
        );
        assert_eq!(
            out.matches("<thead>").count(),
            3,
            "two synthetic + one real thead expected, got: {out}"
        );
        // Two-column header lands before the two-column data row, the
        // three-column header before the three-column one. The middle table
        // keeps its real `<th>` labels and gains no synthetic ones.
        let first_head = out.find("Column 1").expect("first synthetic header");
        let first_row = out.find("<td>a</td>").expect("first data row");
        let second_head = out.find("Column 3").expect("three-column header");
        let third_row = out.find("<td>c</td>").expect("third data row");
        assert!(
            first_head < first_row && second_head < third_row,
            "headers must precede their own tables, got: {out}"
        );
        assert!(
            out.contains("<th>X</th><th>Y</th>"),
            "real headers must survive, got: {out}"
        );
    }

    /// #2602: a ragged table's header is as wide as the widest row, not as
    /// the first row — the grid, not the first row, drives the width.
    #[test]
    fn issue_2602_ragged_table_header_covers_widest_row() {
        let out = with_header(
            "<table><tr><td>a</td><td>b</td></tr>\
             <tr><td>c</td><td>d</td><td>e</td></tr></table>",
        );
        assert!(
            out.contains("<th>Column 1</th><th>Column 2</th><th>Column 3</th>"),
            "header must cover the widest row, got: {out}"
        );
    }

    /// #2602: a `colspan` counts toward the grid width — two slots spelled
    /// as one cell still need two header names.
    #[test]
    fn issue_2602_colspan_counts_toward_header_width() {
        let out = with_header(
            "<table><tr><td colspan=\"2\">a</td><td>b</td></tr>\
             <tr><td>c</td><td>d</td><td>e</td></tr></table>",
        );
        assert!(
            out.contains("<th>Column 1</th><th>Column 2</th><th>Column 3</th>"),
            "header must cover colspan width, got: {out}"
        );
    }

    /// #2602: the fast-path gate is case-insensitive — an uppercase
    /// `<TABLE>` still gets its synthetic header.
    #[test]
    fn issue_2602_uppercase_table_is_synthesized() {
        let out = with_header("<TABLE><tr><td>a</td><td>b</td></tr></TABLE>");
        assert!(
            out.contains("<thead><tr><th>Column 1</th><th>Column 2</th></tr></thead>"),
            "uppercase TABLE must be synthesized, got: {out}"
        );
    }
}
