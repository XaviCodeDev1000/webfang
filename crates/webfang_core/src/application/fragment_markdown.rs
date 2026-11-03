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
pub(crate) fn with_header(fragment: &str) -> String {
    if !fragment.contains("<table") {
        return fragment.to_owned();
    }

    let document = scraper::Html::parse_fragment(fragment);
    let headers: Vec<Option<String>> = document
        .select(&selector("table"))
        .map(|table| header_for_table(&table))
        .collect();
    if headers.iter().all(Option::is_none) {
        return fragment.to_owned();
    }

    use lol_html::html_content::ContentType;
    let position = std::cell::Cell::new(0usize);
    let settings = lol_html::RewriteStrSettings {
        element_content_handlers: vec![lol_html::element!("table", |el| {
            let index = position.get();
            position.set(index + 1);
            if let Some(Some(header)) = headers.get(index) {
                el.prepend(header, ContentType::Html);
            }
            Ok(())
        })],
        ..lol_html::RewriteStrSettings::default()
    };
    lol_html::rewrite_str(fragment, settings).unwrap_or_else(|_| fragment.to_owned())
}

/// The `<thead>` a table needs, or `None` when it must be left alone.
fn header_for_table(table: &scraper::ElementRef<'_>) -> Option<String> {
    // `htmd` already infers the header row when a `<th>` exists anywhere, so
    // there is nothing to synthesize.
    if table.select(&selector("th")).next().is_some() {
        return None;
    }
    let first_row = table.select(&selector("tr")).next()?;
    let columns = first_row
        .children()
        .filter(|cell| {
            cell.value()
                .as_element()
                .is_some_and(|element| matches!(element.name(), "td" | "th"))
        })
        .count();
    if columns < 2 {
        return None;
    }
    let cells: String = (1..=columns)
        .map(|index| format!("<th>Column {index}</th>"))
        .collect();
    Some(format!("<thead><tr>{cells}</tr></thead>"))
}

/// Parse a compile-time-constant selector; the literals below cannot fail.
fn selector(css: &str) -> scraper::Selector {
    #[allow(clippy::expect_used)]
    let parsed = scraper::Selector::parse(css)
        // LCOV_EXCL_LINE defensive: compile-time selector — a constant literal
        .expect("invariant: the selector literals in this module are valid CSS");
    parsed
}

#[cfg(test)]
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
}
