use super::tests_support::{seed_content, test_handler_with_export_roots};
use super::*;
use crate::mcp_server::handlers::test_support::result_text;

/// Issue #1814 (slice B, AC4): `process_results` (sync fs I/O + SHA-256
/// per item + JSONL serialization) must run on the blocking pool, never on
/// the rmcp Tokio worker.
///
/// Deterministic off-runtime proof: a ~80 MiB session (160 x 512 KiB
/// items) makes the export seconds of REAL work at `opt-level = 0`, far
/// above the 1 s scheduling budget below. The tick anchors on the export's
/// ENTRY into
/// `process_results` (its first act is `create_dir_all(output_dir)`) so
/// the earlier session-snapshot `spawn_blocking` window cannot let the
/// tick land before the heavy inline section starts; the budget is
/// measured from task start either way. On a `current_thread` runtime the
/// old inline export monopolized the single worker, so the tick could not
/// complete within the budget and the test failed; with `spawn_blocking`
/// the scheduler stays responsive while the export is still in flight.
#[tokio::test(flavor = "current_thread")]
async fn export_jsonl_keeps_runtime_responsive_during_large_export() {
    use std::time::Duration;

    let (handler, tmp) = test_handler_with_export_roots().await;
    const ITEMS: usize = 160;
    const BODY_BYTES: usize = 512 * 1024;
    {
        let mut guard = handler
            .state
            .session_results
            .lock()
            .expect("fresh session lock is never poisoned");
        for i in 0..ITEMS {
            guard.push(seed_content(
                &format!("https://example.com/heavy-{i}"),
                &format!("Heavy {i}"),
                &format!("item {i} {}", "x".repeat(BODY_BYTES)),
            ));
        }
    }
    let out_dir = tmp.path().join("starve-export");
    let params = ExportJsonlParams {
        output_dir: Some(out_dir.to_string_lossy().into_owned()),
        filename: Some("heavy".to_string()),
    };
    let handler = std::sync::Arc::new(handler);
    let export = tokio::spawn(async move { handler.export_jsonl(Parameters(params)).await });
    let watch_dir = out_dir.clone();
    let tick = tokio::spawn(async move {
        // Anchor: wait until `process_results` has been entered (its first
        // act creates the output directory), then prove the scheduler is
        // still live PAST that entry point.
        loop {
            if watch_dir.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    tokio::time::timeout(Duration::from_secs(1), tick)
        .await
        .expect("current_thread scheduler must stay responsive during a large export")
        .expect("tick task must not panic");
    assert!(
        !export.is_finished(),
        "the large export should still be in flight when the tick lands"
    );

    let res = export
        .await
        .expect("export task joins")
        .expect("export_jsonl returns a tool result");
    let text = result_text(&res);
    assert!(
        text.contains("Export completed"),
        "the large export must still succeed: {text}"
    );
    assert!(
        out_dir.join("heavy.jsonl").exists(),
        "the export file must be written: {text}"
    );
}
