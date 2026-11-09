//! Incremental spool flush (slice 2, WU3 — #2611).
//!
//! The batch captures pages into the spool as they are fetched, but without a
//! flush policy the `.md` conversion happens ONLY at the clean close — the
//! exact behavior that loses 403 captured pages to a kill (#2611's report).
//! This module is the policy: every `N` captured pages (or on graceful
//! shutdown, WU5) the spool tail past the watermark is converted with the
//! SAME pipeline the clean close and `webfang recover` use, written to the
//! output directory, and the sidecar's watermark advances.
//!
//! # Ordering (design S6, per page)
//!
//! `.md` written → COMMITTED record → watermark advanced. A page whose
//! extraction fails still advances the watermark (it is consumed bytes —
//! the clean-close export would fail on it too); an I/O write failure does
//! NOT advance the watermark and blocks the spool's discard, so nothing is
//! lost and nothing is double-exported.
//!
//! # Resume contract (S9, verified assumption)
//!
//! The `content_hash` recorded in a COMMITTED is the SHA-256 of the
//! `ScrapedContent` text the flush wrote — the same field the export's
//! `CommitSession::decide` matches against the `export.jsonl` hash index, so
//! a resume that finds a pre-watermark COMMITTED skips it for this output.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{info, instrument, warn};

use crate::application::batch::progress::{
    BatchProgressV1, ExtractionOptions, BATCH_PROGRESS_VERSION,
};
use crate::application::batch::spool_export::{convert_spool, SpoolExportConfig};
use crate::application::crawler::bounded_sink::CapturedPageReader;
use crate::domain::CorrelationId;
use crate::infrastructure::output::file_saver::{save_results, ObsidianOptions};
use crate::OutputFormat;

/// Result of one incremental flush of the spool tail.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FlushOutcome {
    /// Pages converted AND written in this flush.
    pub exported: usize,
    /// The exported `(url, content)` pairs, for the COMMITTED seam (WU4).
    /// Cleared before returning when the caller has no store, to keep the
    /// flush's memory bounded on huge tails.
    pub pages: Vec<(String, crate::domain::ScrapedContent)>,
    /// Pages whose extraction failed (watermark advanced past them — the
    /// clean close would fail on them identically).
    pub failed: usize,
    /// Corrupt spool lines skipped (consumed bytes, counted).
    pub skipped_corrupt: u64,
    /// The new durable watermark after this flush. Equals the caller's
    /// `from_offset` when the flush exported nothing.
    pub watermark: u64,
}

/// Failures of an incremental flush.
#[derive(Debug, thiserror::Error)]
pub enum FlushError {
    /// The spool tail could not be READ. The watermark is untrustworthy past
    /// the last line boundary — the caller keeps the previous one.
    #[error("no se pudo leer el spool de captura: {source}")]
    SpoolRead {
        /// Underlying I/O error.
        source: std::io::Error,
        /// Pages written before the failure (their `.md` is on disk).
        written: usize,
    },
    /// Writing the converted pages failed. The watermark is untouched: the
    /// next flush re-converts the same tail (idempotent — the saver writes
    /// atomically per file).
    #[error("no se pudieron escribir las páginas exportadas: {0}")]
    OutputWrite(#[source] crate::error::ScraperError),
}

/// Flush the spool tail past `from_offset` into `output_dir`.
///
/// Conversion is the SHARED `convert_spool` path (identical extractor and
/// names as the clean-close export and recover — #1290 one pipeline);
/// writing goes through the repo's single saver on the blocking pool.
///
/// # Errors
///
/// [`FlushError::SpoolRead`] keeps the caller's watermark; a partial write
/// ([`FlushError::OutputWrite`]) leaves it too — re-flushing re-exports the
/// same tail atomically per file.
#[instrument(skip_all, fields(from_offset, output = %output_dir.display()))]
pub async fn flush_spool_tail(
    spool_path: &Path,
    from_offset: u64,
    output_dir: &Path,
    extraction: &ExtractionOptions,
) -> Result<FlushOutcome, FlushError> {
    let cfg = SpoolExportConfig {
        scraper: extraction.to_scraper_config(output_dir),
    };

    let mut reader = CapturedPageReader::open_at(spool_path, from_offset)
        .await
        .map_err(|e| FlushError::SpoolRead {
            source: bounded_error_io(e),
            written: 0,
        })?;
    let outcome = convert_spool(&mut reader, &cfg)
        .await
        .map_err(|e| match e {
            crate::application::batch::spool_export::SpoolExportError::SpoolRead {
                source,
                partial,
            } => FlushError::SpoolRead {
                source,
                written: partial.converted.len(),
            },
            other => FlushError::SpoolRead {
                source: std::io::Error::other(other.to_string()),
                written: 0,
            },
        })?;

    let converted: Vec<crate::domain::ScrapedContent> = outcome
        .converted
        .iter()
        .map(|c| c.content.clone())
        .collect();
    let output = output_dir.to_path_buf();
    let format = OutputFormat::Markdown;
    let obsidian = ObsidianOptions::default();
    tokio::task::spawn_blocking(move || save_results(&converted, &output, &format, &obsidian))
        .await
        .map_err(|join| {
            FlushError::OutputWrite(crate::error::ScraperError::Io(std::io::Error::other(join)))
        })?
        .map_err(FlushError::OutputWrite)?;

    let pages: Vec<(String, crate::domain::ScrapedContent)> = outcome
        .converted
        .iter()
        .map(|c| (c.url.clone(), c.content.clone()))
        .collect();
    Ok(FlushOutcome {
        exported: pages.len(),
        pages,
        failed: outcome.failed.len(),
        skipped_corrupt: outcome.skipped_corrupt,
        watermark: outcome.consumed_bytes,
    })
}

/// Sidecar content for a running batch: identity of the CURRENT spool plus
/// the flush position. Written at startup (offset 0) and advanced by every
/// flush; the next run's acquisition reads it through `decide_sweep`.
#[must_use]
pub fn running_sidecar(
    spool: &crate::application::batch::progress::SpoolIdentity,
    output_dir: &Path,
    extraction: &ExtractionOptions,
    watermark_bytes: u64,
) -> BatchProgressV1 {
    BatchProgressV1 {
        version: BATCH_PROGRESS_VERSION,
        spool: spool.clone(),
        output_dir: output_dir.to_path_buf(),
        extraction_options: extraction.clone(),
        watermark_bytes,
    }
}

/// Persist the sidecar atomically (write temp + rename, the repo's one
/// crash-safe pattern).
///
/// # Errors
///
/// Surfaces I/O failures to the caller — the flush caller logs them and
/// keeps running (a sidecar write failure degrades NEXT-run salvage to
/// OrphanSpool, it does not kill the live run).
pub async fn write_sidecar(root: &Path, sidecar: &BatchProgressV1) -> std::io::Result<()> {
    use crate::application::batch::progress::BATCH_PROGRESS_FILE;

    let path = root.join(BATCH_PROGRESS_FILE);
    let tmp = root.join(format!("{BATCH_PROGRESS_FILE}.tmp"));
    let bytes = serde_json::to_vec(sidecar).map_err(std::io::Error::other)?;
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, &path).await
}

/// Read the sidecar for the salvage decision paths (absent = None).
///
/// # Errors
///
/// Only the file's absence is `None`; anything else propagates.
pub async fn read_sidecar(root: &Path) -> std::io::Result<Option<BatchProgressV1>> {
    use crate::application::batch::progress::BATCH_PROGRESS_FILE;

    match tokio::fs::read(root.join(BATCH_PROGRESS_FILE)).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| std::io::Error::other(format!("sidecar corrupto: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove the sidecar after a clean close: the spool is deleted (discard)
/// or fully exported, so a sidecar describing a dead offset would only
/// mislead the next acquisition into `OrphanSpool` quarantine.
///
/// # Errors
///
/// Absent file is success; real I/O failures propagate.
pub async fn clear_sidecar(root: &Path) -> std::io::Result<()> {
    use crate::application::batch::progress::BATCH_PROGRESS_FILE;

    match tokio::fs::remove_file(root.join(BATCH_PROGRESS_FILE)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Reduce a [`crate::application::crawler::bounded_sink::BoundedSinkError`]
/// to its I/O core (same reduction `spool_export` uses).
fn bounded_error_io(
    e: crate::application::crawler::bounded_sink::BoundedSinkError,
) -> std::io::Error {
    use crate::application::crawler::bounded_sink::BoundedSinkError;
    match e {
        BoundedSinkError::Io(io) => io,
        other => std::io::Error::other(other),
    }
}

/// Commit one flush's exported pages into the resume store so the next run
/// (or the clean-close export) skips them as `COMMITTED`-proven.
///
/// # S9 resolution (verified against `CommitSession::decide`)
///
/// `decide` matches `record.content_hash` against the `export.jsonl`
/// `checksum_sha256` index, and the exporter stamps that checksum as
/// **SHA-256 of the `ScrapedContent` text** (`process_single_item`) — NOT
/// of the `.md` file bytes (frontmatter would change the digest). So the
/// flush records `sha256(content)`, identical to the clean-close path, and
/// a flush-committed page skips via its `COMMITTED` status before any hash
/// comparison anyway. The plan's S9 assumption (hash of the `.md` file) is
/// corrected here: the `.md` bytes are not the identity the resume gate
/// compares.
///
/// Runs on the blocking pool: `commit_item` → `merge_persist` takes the
/// store's `flock` and does file I/O.
pub fn commit_flushed_pages(
    store: &crate::infrastructure::export::RecordStore,
    pages: &[(String, crate::domain::ScrapedContent)],
    output_path: &Path,
) {
    use crate::application::export_factory::{CommitSession, ResumeContext};
    use crate::application::resume::RunId;

    let ctx = ResumeContext::new(store).with_resume(true);
    let run_id = RunId::new();
    let mut session = CommitSession::open(Some(&ctx), output_path);
    for (url, content) in pages {
        // Same digest the JSONL exporter stamps into `checksum_sha256`.
        let content_hash = format!("{:x}", Sha256::digest(content.content.as_bytes()));
        session.commit_item(url, content_hash, output_path.to_path_buf(), &run_id);
    }
    session.final_persist();
}

/// Async wrapper for [`commit_flushed_pages`] on the blocking pool.
pub async fn commit_flushed_pages_async(
    store: crate::infrastructure::export::RecordStore,
    pages: Vec<(String, crate::domain::ScrapedContent)>,
    output_path: PathBuf,
) {
    let _ = tokio::task::spawn_blocking(move || commit_flushed_pages(&store, &pages, &output_path))
        .await
        .map_err(|join| {
            warn!(error = %join, "flush commit task failed — pages stay EXPORTED-not-COMMITTED and re-drive at close")
        });
}

/// Compute the run's batch namespace: `batch-<hash8(canonical output key)>`.
///
/// The namespace separates concurrent/resumed batches targeting DIFFERENT
/// output directories from sharing (and wrongly skipping against) each
/// other's COMMITTED records (L3). The canonical output key is the resolved
/// persistence root as given — same string the state store derives domains
/// from.
#[must_use]
pub fn batch_namespace(output_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    output_dir.hash(&mut hasher);
    format!("batch-{:08x}", hasher.finish())
}

/// Log one flush in the shared structured shape (operator summary).
#[must_use]
pub fn log_flush(outcome: &FlushOutcome, correlation: &CorrelationId) -> String {
    let summary = format!(
        "Checkpoint: {} páginas exportadas, {} fallidas, {} corruptas (watermark {})",
        outcome.exported, outcome.failed, outcome.skipped_corrupt, outcome.watermark
    );
    info!(
        exported = outcome.exported,
        failed = outcome.failed,
        skipped_corrupt = outcome.skipped_corrupt,
        watermark = outcome.watermark,
        correlation = %correlation,
        "incremental spool flush completed"
    );
    if outcome.failed > 0 {
        warn!(
            failed = outcome.failed,
            "pages failed extraction during flush — their bytes are consumed and counted"
        );
    }
    summary
}

/// Safety tick between watermark polls while the crawl runs (WU3).
pub const FLUSH_WATCH_TICK: std::time::Duration = std::time::Duration::from_secs(120);

/// Grace period the final flush gets on shutdown before the run proceeds
/// anyway (design S6: cierre con timeout 30 s y mensaje de spool conservado).
pub const FLUSH_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// One sidecar update: identity + watermark, persisted atomically.
async fn persist_sidecar(
    root: &Path,
    identity: &crate::application::batch::progress::SpoolIdentity,
    output_dir: &Path,
    extraction: &ExtractionOptions,
    watermark: u64,
) {
    let sidecar = running_sidecar(identity, output_dir, extraction, watermark);
    if let Err(e) = write_sidecar(root, &sidecar).await {
        // A sidecar write failure degrades NEXT-run salvage to OrphanSpool
        // quarantine; it must never kill the live run.
        warn!(error = %e, "batch progress sidecar could not be written");
    }
}

/// Everything the flush watch needs, bundled (the function had 8 args,
/// past the clippy threshold — one struct keeps the call site readable).
pub struct FlushWatchParams {
    /// The live capture sink (its `captured()` / `durable_bytes()` drive the
    /// loop).
    pub sink: std::sync::Arc<crate::application::crawler::bounded_sink::BoundedFileSink>,
    /// Identity of the spool this run is writing.
    pub spool_identity: crate::application::batch::progress::SpoolIdentity,
    /// Persistence root holding the spool + sidecar.
    pub root: PathBuf,
    /// Output directory the flush exports into.
    pub output_dir: PathBuf,
    /// Extraction options replayed on the tail (recorded in the sidecar).
    pub extraction: ExtractionOptions,
    /// Pages-between-flushes threshold (`--export-checkpoint`).
    pub checkpoint: u64,
    /// Run-root identity for the structured flush logs.
    pub correlation: CorrelationId,
    /// Fired by the caller when the crawl ends (the loop then does its final
    /// bounded flush and exits).
    pub done: tokio_util::sync::CancellationToken,
    /// Resume store (batch-namespace) the flush COMMITTEDs into, when the
    /// checkpoint path built one.
    pub store: Option<crate::infrastructure::export::RecordStore>,
}

/// Run the checkpoint flush loop alongside the crawl (WU3, #2611).
///
/// Every time `captured_now - captured_flushed >= checkpoint` (polled on the
/// safety tick), the spool tail past the watermark is converted + written and
/// the sidecar advances. Ends when `done` fires; the CALLER then runs the
/// final flush under [`FLUSH_SHUTDOWN_TIMEOUT`] — this task never races the
/// sink's own `finish()`.
///
/// Failure policy: a flush error is logged and retried on the next threshold
/// crossing; the watermark stays where the last SUCCESSFUL flush left it, so
/// no page is lost or double-exported (S6 ordering).
pub async fn run_flush_watch(p: FlushWatchParams) {
    let FlushWatchParams {
        sink,
        spool_identity,
        root,
        output_dir,
        extraction,
        checkpoint,
        correlation,
        done,
        store,
    } = p;
    let spool_path = root.join(crate::cli::batch_flow::BATCH_SPOOL_FILE);
    let watermark: std::sync::Arc<std::sync::atomic::AtomicU64> =
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut interval = tokio::time::interval(FLUSH_WATCH_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = done.cancelled() => break,
            _ = interval.tick() => {
                let captured = u64::try_from(sink.captured()).unwrap_or(u64::MAX);
                let flushed = watermark.load(std::sync::atomic::Ordering::Acquire);
                if checkpoint == 0 || captured.saturating_sub(flushed) < checkpoint {
                    continue;
                }
                // The writer's durable watermark is the only safe conversion
                // point: pages still in the channel/backlog are NOT on disk.
                let durable = sink.durable_bytes().max(flushed);
                match flush_spool_tail(&spool_path, flushed, &output_dir, &extraction).await {
                    Ok(outcome) => {
                        // Clamp to what the writer vouches for: pages converted
                        // from bytes BEYOND the durable watermark would mark
                        // unflushed data as exported.
                        let new_watermark = outcome.watermark.min(durable);
                        watermark.store(new_watermark, std::sync::atomic::Ordering::Release);
                        let _ = log_flush(&outcome, &correlation);
                        commit_flush(&store, &output_dir, &outcome).await;
                        persist_sidecar(&root, &spool_identity, &output_dir, &extraction, new_watermark).await;
                    },
                    Err(e) => {
                        warn!(error = %e, watermark = flushed, "checkpoint flush failed — will retry on the next threshold crossing");
                    },
                }
            },
        }
    }

    // Crawl finished (or shutdown requested): one LAST flush of everything
    // the writer made durable, bounded by the shutdown timeout. Runs on the
    // blocking pool so a slow disk cannot wedge the async runtime.
    let flushed = watermark.load(std::sync::atomic::Ordering::Acquire);
    let durable = sink.durable_bytes().max(flushed);
    if durable <= flushed {
        return;
    }
    let attempt = {
        let spool_path = spool_path.clone();
        let output_dir = output_dir.clone();
        let extraction = extraction.clone();
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(async move {
                flush_spool_tail(&spool_path, flushed, &output_dir, &extraction).await
            })
        })
    };
    match tokio::time::timeout(FLUSH_SHUTDOWN_TIMEOUT, attempt).await {
        Ok(Ok(Ok(outcome))) => {
            let new_watermark = outcome.watermark.min(durable);
            let _ = log_flush(&outcome, &correlation);
            commit_flush(&store, &output_dir, &outcome).await;
            persist_sidecar(
                &root,
                &spool_identity,
                &output_dir,
                &extraction,
                new_watermark,
            )
            .await;
        },
        Ok(Ok(Err(e))) => {
            log_final_flush_failure(&spool_path, flushed, &e);
        },
        Ok(Err(join)) => {
            let e = FlushError::OutputWrite(crate::error::ScraperError::Io(std::io::Error::other(
                join.to_string(),
            )));
            log_final_flush_failure(&spool_path, flushed, &e);
        },
        Err(_) => {
            // The 30 s budget ran out: the spool is CONSERVED (never deleted
            // on a failed close) and the operator is told where their data is.
            log_final_flush_failure(
                &spool_path,
                flushed,
                &FlushError::SpoolRead {
                    source: std::io::Error::other("el flush final excedió el timeout de 30 s"),
                    written: 0,
                },
            );
        },
    }
}

/// COMMITTED-seam for one flush (WU4): persist every exported page into the
/// batch-namespace store so the clean close and a `--resume` relaunch skip
/// it. Order per page is the design's `.md` → COMMITTED → watermark; the
/// watermark advance happens in the caller after this returns.
async fn commit_flush(
    store: &Option<crate::infrastructure::export::RecordStore>,
    output_dir: &Path,
    outcome: &FlushOutcome,
) {
    let Some(store) = store else { return };
    if outcome.pages.is_empty() {
        return;
    }
    // output_location points at the RAG export the clean close appends to;
    // the flush itself wrote `.md` files, and the record's status (not the
    // hash index) is what makes the close skip the page.
    let output_path = output_dir.join("export.jsonl");
    commit_flushed_pages_async(store.clone(), outcome.pages.clone(), output_path).await;
}

/// The honest close failure: the spool is CONSERVED and the operator is told
/// where their data is and how to turn it back into markdown.
fn log_final_flush_failure(spool_path: &Path, watermark: u64, error: &FlushError) {
    warn!(
        error = %error,
        spool = %spool_path.display(),
        watermark,
        user_message = "El cierre de la corrida no pudo exportar todo el spool pendiente. \
    El archivo se conserva; recuperá las páginas con: webfang recover --spool <ruta> -o <dir>",
        "final flush did not complete — spool conserved"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::batch::progress::SpoolIdentity;

    fn substantial_html(title: &str) -> String {
        // The shared minimum-content guard (#706) rejects tiny pages, so
        // fixtures carry real bodies to exercise the exported path.
        let body = "contenido de prueba sustancial para superar el guardia de contenido mínimo. "
            .repeat(12);
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

    #[tokio::test]
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
}
