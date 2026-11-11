use super::*;
use crate::application::batch::progress::SpoolIdentity;

fn substantial_html(title: &str) -> String {
    // The shared minimum-content guard (#706) rejects tiny pages, so
    // fixtures carry real bodies to exercise the exported path.
    let body =
        "contenido de prueba sustancial para superar el guardia de contenido mínimo. ".repeat(12);
    format!("<html><body><h1>{title}</h1><p>{body}</p></body></html>")
}

async fn seed_spool(dir: &tempfile::TempDir, urls: &[&str]) -> u64 {
    let mut body = String::new();
    for url in urls {
        let page = crate::application::crawler::content_sink::CapturedPage {
            url: (*url).to_string(),
            html: substantial_html(url),
        };
        body.push_str(&serde_json::to_string(&page).expect("serialize"));
        body.push('\n');
    }
    let path = dir.path().join(".webfang-batch-capture.jsonl");
    tokio::fs::write(&path, &body).await.expect("write spool");
    u64::try_from(body.len()).expect("len")
}

fn options() -> ExtractionOptions {
    ExtractionOptions {
        selector: Some("body".to_string()),
        ignore_waf: false,
    }
}

// Every flush that converts a real page reaches
// `flush_spool_tail → convert_spool → extract_content → extract_with_selector →
// serialize_matches → absolutize_urls → lol_html::rewrite_str`, where servo_arc
// raises the Tree-Borrows UB (tracker servo/stylo#210). What these three tests
// verify is the TAIL FLUSH — the watermark, the offset window, the retry
// contract — never the rewritten HTML, so the UB buys them nothing.
#[tokio::test]
#[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB (stylo#210)")]
async fn flush_converts_and_writes_the_full_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = tempfile::tempdir().expect("outdir");
    let total = seed_spool(&dir, &["https://a.test/x", "https://a.test/y"]).await;

    let outcome = flush_spool_tail(
        &dir.path().join(".webfang-batch-capture.jsonl"),
        0,
        out.path(),
        &options(),
    )
    .await
    .expect("flush");

    assert_eq!(outcome.exported, 2);
    assert_eq!(outcome.failed, 0);
    assert_eq!(outcome.skipped_corrupt, 0);
    assert_eq!(outcome.watermark, total, "full tail consumed");
    assert!(
        out.path().join("x.md").exists() || out.path().read_dir().expect("out").count() > 0,
        "markdown files land in the output directory"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB (stylo#210)")]
async fn flush_from_offset_only_exports_the_new_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out1 = tempfile::tempdir().expect("out1");
    let out2 = tempfile::tempdir().expect("out2");
    let path = dir.path().join(".webfang-batch-capture.jsonl");

    let first_tail = seed_spool(&dir, &["https://a.test/first"]).await;
    let first = flush_spool_tail(&path, 0, out1.path(), &options())
        .await
        .expect("first flush");
    assert_eq!(first.exported, 1);
    assert_eq!(first.watermark, first_tail);

    // Append the second page (the writer's steady state) and flush ONLY
    // the new bytes.
    let second = crate::application::crawler::content_sink::CapturedPage {
        url: "https://a.test/second".to_string(),
        html: substantial_html("second"),
    };
    let mut body = tokio::fs::read(&path).await.expect("read");
    body.extend_from_slice(serde_json::to_string(&second).expect("ser").as_bytes());
    body.push(b'\n');
    tokio::fs::write(&path, &body).await.expect("append");

    let second_flush = flush_spool_tail(&path, first.watermark, out2.path(), &options())
        .await
        .expect("second flush");
    assert_eq!(second_flush.exported, 1, "only the appended page");
    assert_eq!(
        second_flush.watermark,
        body.len() as u64,
        "watermark reaches the new EOF"
    );
    assert_eq!(out2.path().read_dir().expect("out2").count(), 1);
}

#[tokio::test]
#[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB (stylo#210)")]
async fn flush_failure_keeps_the_watermark_for_a_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".webfang-batch-capture.jsonl");
    let total = seed_spool(&dir, &["https://a.test/x"]).await;

    // Output "directory" is a FILE: the saver's create_dir_all fails.
    let not_a_dir = dir.path().join("blocker");
    tokio::fs::write(&not_a_dir, b"").await.expect("blocker");

    let err = flush_spool_tail(&path, 0, &not_a_dir, &options())
        .await
        .expect_err("write into a file path must fail");

    assert!(matches!(err, FlushError::OutputWrite(_)), "got {err:?}");
    // The spool is untouched: the caller's watermark stays valid.
    let len = tokio::fs::metadata(&path).await.expect("spool").len();
    assert_eq!(len, total);
}

#[tokio::test]
async fn sidecar_round_trips_and_clears() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sidecar = running_sidecar(
        &SpoolIdentity {
            first_line_sha256: Some("deadbeef".to_string()),
        },
        dir.path(),
        &options(),
        42,
    );

    write_sidecar(dir.path(), &sidecar).await.expect("write");
    let read = read_sidecar(dir.path())
        .await
        .expect("read")
        .expect("present");
    assert_eq!(read, sidecar);

    clear_sidecar(dir.path()).await.expect("clear");
    assert!(read_sidecar(dir.path()).await.expect("read").is_none());
    // Clearing twice is success (absent = ok).
    clear_sidecar(dir.path()).await.expect("idempotent clear");
}

#[test]
fn batch_namespace_is_stable_per_output_dir_and_differs_across() {
    let a = batch_namespace(Path::new("/tmp/vault-a"));
    let b = batch_namespace(Path::new("/tmp/vault-b"));
    assert_eq!(a, batch_namespace(Path::new("/tmp/vault-a")));
    assert_ne!(a, b, "different output dirs must not share a namespace");
    assert!(a.starts_with("batch-"));
}
