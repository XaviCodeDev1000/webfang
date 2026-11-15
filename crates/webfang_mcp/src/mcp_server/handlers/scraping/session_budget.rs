/// `operation` field of every event the session-result budget emits — the
/// resource it governs (same convention as the session cap's
/// `SESSION_OPERATION` in `server.rs`).
pub(super) const SESSION_RESULT_OPERATION: &str = "mcp.session.results";

/// Bytes one retained record occupies in the session buffer (#1611, F7).
///
/// Deliberately a LOWER bound on the record's true footprint, never an
/// estimate of it: it sums every owned string (plus the assets' recorded file
/// sizes), so the accounting cannot be defeated by a record that is large in
/// some field this function forgot. Under-counting is the safe direction for a
/// bound whose job is to refuse an oversized run, because the ceiling has
/// three orders of magnitude of headroom over a default crawl.
pub(super) fn retained_record_bytes(content: &webfang_core::domain::ScrapedContent) -> usize {
    let strings = content.title.len()
        + content.content.len()
        + content.url.as_str().len()
        + content.excerpt.as_ref().map_or(0, String::len)
        + content.author.as_ref().map_or(0, String::len)
        + content.date.as_ref().map_or(0, String::len)
        + content.html.as_ref().map_or(0, String::len);
    let assets: usize = content
        .assets
        .iter()
        .map(|a| a.url.len() + a.local_path.len() + a.size as usize)
        .sum();
    strings.saturating_add(assets)
}

/// Why a run's records were refused by the session budget (#1611, F7).
///
/// Carries the numbers a client needs to fix the call — the budget it blew,
/// how many records were retained, how many were dropped, and how big the run
/// actually was — because the only useful advice here is "crawl fewer pages",
/// and that advice is worthless without the size it was measured against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionResultOverflow {
    /// The ceiling that was exceeded.
    pub budget_bytes: usize,
    /// Records that fit inside the budget.
    pub kept: usize,
    /// Records that did not.
    pub dropped: usize,
    /// Bytes the run's records actually occupied.
    pub total_bytes: usize,
}

/// Spanish operator-facing sentence for an over-budget run.
///
/// Channel B, so it is what an agent reads verbatim (see
/// `docs/src/mcp-error-contract.md`, row B3.11): it names the budget, says
/// plainly that NOTHING of this run was retained, and gives the one lever that
/// actually works — a smaller `max_pages`, or a `crawl_site` per section.
pub(super) fn session_budget_message(overflow: &SessionResultOverflow) -> String {
    format!(
        "El crawl produjo {} registros ({:.1} MiB) y supera el presupuesto de resultados de sesión de {} bytes: no se retuvo ninguno. \
         Repita el rastreo con menos páginas (`max_pages`) o exporte por secciones.",
        overflow.kept + overflow.dropped,
        overflow.total_bytes as f64 / (1024.0 * 1024.0),
        overflow.budget_bytes,
    )
}

/// Fit one run's records into a session budget, or refuse the whole run
/// (#1611, F7).
///
/// All-or-nothing on purpose. A partial buffer would make the export tools
/// serve a set that silently disagrees with the crawl they followed, and the
/// error contract's rule — "if the tool could not produce the answer it
/// promised, the answer is `isError: true`, never a success whose body
/// happens to read like a failure" — treats a truncated export as a lost
/// report, not a smaller one. Refusing the run also keeps the caller from
/// having to reason about which half survived.
///
/// A single record larger than the whole budget is refused too (`kept == 0`),
/// rather than being admitted as an over-budget exception: a budget that one
/// record can breach is not a budget.
pub(crate) fn within_session_budget(
    records: Vec<webfang_core::domain::ScrapedContent>,
    budget_bytes: usize,
) -> Result<Vec<webfang_core::domain::ScrapedContent>, SessionResultOverflow> {
    let total_bytes: usize = records.iter().map(retained_record_bytes).sum();
    if total_bytes <= budget_bytes {
        return Ok(records);
    }
    let kept = records
        .iter()
        .scan(0usize, |acc, record| {
            *acc += retained_record_bytes(record);
            (*acc <= budget_bytes).then_some(())
        })
        .count();
    Err(SessionResultOverflow {
        budget_bytes,
        kept,
        dropped: records.len() - kept,
        total_bytes,
    })
}
