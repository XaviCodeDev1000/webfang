use super::{build_batch_sink, observe_spool_pair, quarantine_orphan_spool, BATCH_SPOOL_FILE};
use crate::application::batch::progress::{
    BatchProgressV1, ExtractionOptions, SidecarState, SpoolIdentity, BATCH_PROGRESS_FILE,
};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;

/// Run an async closure under the SHARED capturing subscriber
/// ([`crate::infrastructure::observability::test_log_capture::LogBuffer`] —
/// the same helper every other test module uses, so the capture writer is
/// written once in the crate) and return what it logged plus the value.
async fn capture_logs<T, F, Fut>(f: F) -> (String, T)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    use crate::infrastructure::observability::test_log_capture::LogBuffer;
    let (buffer, _guard) = LogBuffer::capture();
    let value = f().await;
    (buffer.text(), value)
}

// --- Slice 2 (WU1): the unified spool acquisition (observe → decide → act).

/// Seed a sidecar describing the CURRENT first line of the spool at `dir`.
async fn seed_matching_sidecar(dir: &tempfile::TempDir, watermark: u64) {
    let spool_bytes = tokio::fs::read(dir.path().join(BATCH_SPOOL_FILE))
        .await
        .expect("read spool");
    let first_line = crate::application::batch::progress::spool_first_line_sha256(&spool_bytes);
    let sidecar = BatchProgressV1 {
        version: 1,
        spool: SpoolIdentity {
            first_line_sha256: first_line,
        },
        output_dir: dir.path().join("out"),
        extraction_options: ExtractionOptions {
            selector: Some("body".to_string()),
            ignore_waf: false,
        },
        watermark_bytes: watermark,
    };
    tokio::fs::write(
        dir.path().join(BATCH_PROGRESS_FILE),
        serde_json::to_vec(&sidecar).expect("serialize sidecar"),
    )
    .await
    .expect("write sidecar");
}

/// `observe_spool_pair` must feed the pure table EXACTLY what is on disk:
/// a matching sidecar parses, a corrupt one is Unreadable, and the spool
/// identity is the first-line hash.
#[tokio::test]
async fn observe_reads_sidecar_state_and_spool_identity() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    tokio::fs::write(dir.path().join(BATCH_SPOOL_FILE), b"{\"url\":\"a\"}\nrest")
        .await
        .expect("seed spool");

    // Absent sidecar.
    let (sidecar, spool) = observe_spool_pair(dir.path()).await;
    assert_eq!(sidecar, SidecarState::Absent);
    assert!(
        spool.first_line_sha256.is_some(),
        "a full line has identity"
    );
    assert_eq!(
        spool.len_bytes,
        b"{\"url\":\"a\"}\nrest".len() as u64,
        "the observed length is the real file length"
    );

    // Corrupt sidecar → Unreadable (NOT absent — the distinction is the
    // whole point of the table).
    tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
        .await
        .expect("seed bad sidecar");
    let (sidecar, _) = observe_spool_pair(dir.path()).await;
    assert_eq!(sidecar, SidecarState::Unreadable);

    // Valid sidecar → Parsed with its content.
    seed_matching_sidecar(&dir, 7).await;
    let (sidecar, _) = observe_spool_pair(dir.path()).await;
    assert!(
        matches!(sidecar, SidecarState::Parsed(_)),
        "got {sidecar:?}"
    );
}

/// An orphan spool is renamed aside — never deleted — and the disclosure
/// names the path, the size and the recover remedy in Spanish.
#[tokio::test]
async fn an_unattributable_spool_is_quarantined_and_disclosed() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let spool = dir.path().join(BATCH_SPOOL_FILE);
    let body = "{\"url\":\"https://a.example\",\"body\":\"PAGE\"}\n".repeat(8);
    tokio::fs::write(&spool, &body)
        .await
        .expect("seed leftover");
    // Unreadable sidecar + non-empty spool = OrphanSpool by the table.
    tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
        .await
        .expect("seed bad sidecar");

    let (logs, orphan) = capture_logs(|| {
        let spool = spool.clone();
        async move { quarantine_orphan_spool(&spool).await }
    })
    .await;

    let (orphan_path, orphan_bytes) = orphan.expect("the orphan must be moved aside");
    assert!(!spool.exists(), "the original path is free for the new run");
    assert!(orphan_path.exists(), "the data survives under the new name");
    assert_eq!(
        orphan_bytes,
        body.len() as u64,
        "the disclosure must size the residue"
    );
    assert_eq!(
        orphan_path.parent(),
        spool.parent(),
        "the orphan stays in the same directory"
    );
    assert!(orphan_path.to_string_lossy().contains(".orphaned"));
    assert!(logs.contains("no se puede atribuir"), "{logs}");
    assert!(logs.contains("NO se eliminó"), "{logs}");
    assert!(logs.contains("webfang recover"), "{logs}");
}

/// A missing spool is not an orphan: the acquisition is a no-op.
#[tokio::test]
async fn a_missing_spool_quarantines_nothing() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let orphan = quarantine_orphan_spool(&dir.path().join(BATCH_SPOOL_FILE)).await;
    assert!(orphan.is_none(), "nothing on disk, nothing to quarantine");
}

/// The full acquisition on a CLEAN root (no spool, no sidecar) must let a
/// fresh run start: the sink is built and the spool path is the original.
#[tokio::test]
async fn acquisition_on_a_clean_root_starts_a_fresh_sink() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = dir.path().to_path_buf();

    let sink = build_batch_sink(&opts).await.expect("sink on clean root");
    assert_eq!(
        sink.spool_path(),
        dir.path().join(BATCH_SPOOL_FILE),
        "the spool lands at the canonical path"
    );
}

/// An orphan whose rename fails must ABORT the run rather than truncate:
/// truncating the spool path would destroy unattributable data (L5).
#[tokio::test]
#[cfg(unix)]
async fn an_unmovable_orphan_fails_the_run_instead_of_truncating() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let spool = dir.path().join(BATCH_SPOOL_FILE);
    tokio::fs::write(&spool, b"unattributable data")
        .await
        .expect("seed");
    tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
        .await
        .expect("bad sidecar");
    // Read-only directory: the rename out must fail.
    let mut perms = std::fs::metadata(dir.path()).expect("dir").permissions();
    perms.set_mode(0o500);
    std::fs::set_permissions(dir.path(), perms).expect("chmod");

    let mut opts = CrawlOptions::default();
    opts.export.output_dir = dir.path().to_path_buf();
    let exit = build_batch_sink(&opts).await;

    // Restore before asserting so a failure never leaves a locked dir.
    let mut perms = std::fs::metadata(dir.path()).expect("dir").permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(dir.path(), perms).expect("restore");

    assert!(
        matches!(exit, Err(CliExit::IoError(_))),
        "an unmovable orphan must fail the run, got {exit:?}"
    );
    let kept = tokio::fs::read(&spool).await.expect("spool intact");
    assert_eq!(
        kept, b"unattributable data",
        "the unattributable data must survive byte for byte"
    );
}
