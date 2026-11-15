use super::session_budget::{retained_record_bytes, session_budget_message};
use super::tests_common::{test_state, ARTICLE_HTML};
use super::*;

// ── #1611 F7: the session-result budget ──

/// Fixture record of a known size class: `content` is the only field that
/// varies, so a test can state "three 1 KiB records" and have the byte sum
/// be about that.
fn budget_record(url: &str, body_bytes: usize) -> webfang_core::domain::ScrapedContent {
    webfang_core::domain::ScrapedContent {
        title: "t".to_string(),
        content: "c".repeat(body_bytes),
        url: webfang_core::domain::ValidUrl::try_from_url(
            url::Url::parse(url).expect("valid fixture url"),
        )
        .expect("fixture url is https"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: None,
    }
}

/// The bound the F7 finding names: the buffer a run leaves behind must
/// fit the budget, and a run that does not is refused as a WHOLE with the
/// numbers a caller needs to shrink it — never truncated into a partial
/// export that would read as a complete one.
#[test]
fn a_run_within_budget_is_retained_whole() {
    let records = vec![
        budget_record("https://example.com/a", 1000),
        budget_record("https://example.com/b", 1000),
    ];
    let total: usize = records.iter().map(retained_record_bytes).sum();
    let admitted =
        within_session_budget(records, total).expect("a run whose bytes equal the budget fits");
    assert_eq!(admitted.len(), 2, "an exact fit is admitted, not refused");
}

#[test]
fn a_run_over_budget_is_refused_whole_and_reports_the_size() {
    let records = vec![
        budget_record("https://example.com/a", 1000),
        budget_record("https://example.com/b", 1000),
        budget_record("https://example.com/c", 1000),
    ];
    let one = retained_record_bytes(&budget_record("https://example.com/x", 1000));
    // Room for two records, three were produced.
    let overflow =
        within_session_budget(records, one * 2).expect_err("a run over the budget must be refused");
    assert_eq!(overflow.budget_bytes, one * 2);
    assert_eq!(overflow.dropped, 1, "the record that did not fit is named");
    assert_eq!(overflow.kept, 2, "the records that fitted are counted");
    assert!(
        overflow.total_bytes > overflow.budget_bytes,
        "the measured size is what makes the advice actionable: {overflow:?}"
    );
}

/// A budget smaller than ONE record still refuses. Without this the bound
/// would be advisory exactly when it matters most — a single enormous page
/// would be admitted as an over-budget exception.
#[test]
fn one_record_larger_than_the_budget_is_refused() {
    let overflow = within_session_budget(vec![budget_record("https://example.com/a", 4096)], 16)
        .expect_err("an oversize single record must be refused");
    assert_eq!(overflow.kept, 0);
    assert_eq!(overflow.dropped, 1);
}

/// The default budget is a real ceiling, not a token one: it must be
/// small enough to bound a process and large enough that no default run
/// hits it.
#[test]
fn the_default_session_budget_is_bounded_and_generous() {
    assert_eq!(
        crate::mcp_server::state::DEFAULT_SESSION_RESULT_BUDGET_BYTES,
        64 * 1024 * 1024
    );
    // `crawl_site` defaults to 100 pages; 100 × 1 MiB is above the
    // budget, which is the point — a run of maximally large pages is
    // refused rather than parked. The common case (tens of KB per page)
    // sits two orders of magnitude below.
    let hundred_mib = 100 * 1024 * 1024;
    assert!(hundred_mib > crate::mcp_server::state::DEFAULT_SESSION_RESULT_BUDGET_BYTES);
}

/// The bound is enforced END TO END through the real capture path: a
/// captured page that converts to a record larger than the session budget
/// is refused, the buffer is left EMPTY (not holding the previous run), and
/// the error carries the numbers. An empty run is the control: it fits any
/// budget and must not error, or the bound would refuse honest clients.
#[tokio::test]
#[cfg_attr(miri, ignore = "Container::new creates HttpClient with btls-sys FFI")]
async fn store_session_results_enforces_the_budget_from_the_state() {
    use webfang_core::application::crawler::content_sink::{CrawlContentSink, InMemoryContentSink};

    let (state, _tmp) = test_state().await;
    // Seed the buffer so the "previous run" case is observable. Scoped so
    // no guard is live across the awaits below.
    {
        let mut guard = state
            .session_results
            .lock()
            .expect("fresh session lock is never poisoned");
        guard.push(budget_record("https://example.com/previous", 16));
    }

    let state = state.with_session_result_budget(std::num::NonZeroUsize::new(1).expect("non-zero"));
    let handler = McpHandler::new(state);

    let empty = InMemoryContentSink::new();
    handler
        .store_session_results(&empty)
        .await
        .expect("an empty run fits any budget");

    let sink = InMemoryContentSink::new();
    sink.capture("https://example.com/page", ARTICLE_HTML);
    let overflow = handler
        .store_session_results(&sink)
        .await
        .expect_err("a record over a one-byte budget must be refused");
    assert_eq!(overflow.budget_bytes, 1);
    assert_eq!(overflow.kept, 0, "nothing fits a one-byte budget");
    assert_eq!(overflow.dropped, 1);
    assert!(
        handler
            .state
            .session_results
            .lock()
            .expect("lock")
            .is_empty(),
        "a refused run must not leave the previous run's records behind: \
         an export would then serve a crawl the client never ran"
    );

    let message = session_budget_message(&overflow);
    assert!(
        message.contains("presupuesto"),
        "the message states what happened: {message}"
    );
    assert!(
        message.contains("max_pages"),
        "the message names the lever that works: {message}"
    );
}
