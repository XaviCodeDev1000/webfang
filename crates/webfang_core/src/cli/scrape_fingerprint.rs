//! Extraction-failure fingerprint wiring (#792 Slice B) for the CLI scrape
//! flow: building the opt-in repository and recording best-effort
//! fingerprints. Split from `scrape_flow.rs`; its call sites there import
//! the two helpers from this child module.

use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::ScrapedContent;

/// Build the fingerprint repository for this run (#792 Slice B).
///
/// Returns `None` unless `--extraction-fingerprint` is set — recording is
/// opt-in. With the `persistence` feature the sink is the shared SQLite DB
/// (`~/.webfang/crawl.db`, overridable via `--db-path`/`WEBFANG_DB_PATH`);
/// without it the flag degrades to a no-op sink with a one-time warning.
/// A pool/schema failure also degrades to no-op: fingerprinting must never
/// abort a scrape run.
pub(super) async fn build_fingerprint_repo(
    opts: &CrawlOptions,
) -> Option<std::sync::Arc<dyn crate::domain::fingerprint_repository::FingerprintRepository>> {
    if !opts.extraction_fingerprint {
        return None;
    }

    #[cfg(feature = "persistence")]
    {
        use crate::infrastructure::autotuning::{env_db_path, resolve_db_path};
        use crate::infrastructure::persistence::{create_pool, SqliteFingerprintRepository};

        let db_path = resolve_db_path(opts.elastic.db_path.as_deref(), env_db_path());
        match create_pool(&db_path, 1) {
            Ok(pool) => {
                let repo = SqliteFingerprintRepository::new(pool);
                match repo.setup_schema().await {
                    Ok(()) => {
                        tracing::info!(
                        db_path = %db_path.display(),
                        "extraction_fingerprint_sink_wired"
                        );
                        return Some(std::sync::Arc::new(repo));
                    },
                    Err(e) => {
                        tracing::warn!(
                        error = %e,
                        "extraction fingerprint schema init failed — degrading to no-op"
                        );
                    },
                }
            },
            Err(e) => {
                tracing::warn!(
                error = %e,
                "extraction fingerprint pool creation failed — degrading to no-op"
                );
            },
        }
        Some(std::sync::Arc::new(
            crate::infrastructure::fingerprint::NoopFingerprintRepository,
        ))
    }

    #[cfg(not(feature = "persistence"))]
    {
        tracing::warn!(
            "--extraction-fingerprint requires the `persistence` feature — degrading to no-op"
        );
        Some(std::sync::Arc::new(
            crate::infrastructure::fingerprint::NoopFingerprintRepository,
        ))
    }
}

/// Record an extraction failure fingerprint when a low-quality extraction
/// produced an honest hint, and attach the accumulated failure count back to
/// the hint (#792 Slice B).
///
/// Best-effort: a persistence error is logged and swallowed — fingerprinting
/// is observability, never a data-path failure.
pub(super) async fn record_extraction_fingerprint(
    url: &Url,
    repo: Option<&dyn crate::domain::fingerprint_repository::FingerprintRepository>,
    selector: &str,
    content: &mut ScrapedContent,
) {
    let Some(repo) = repo else {
        return;
    };
    let Some(hint) = content.quality_hint.as_mut() else {
        return;
    };

    let site_base_url = url.origin().ascii_serialization();
    let selector_signature = selector.to_owned();
    let record = crate::domain::extraction_quality::FingerprintRecord {
        site_base_url: site_base_url.clone(),
        selector_signature: selector_signature.clone(),
        score_at_failure: hint.score.total,
        failure_count: 1,
        last_seen: chrono::Utc::now().timestamp(),
        last_note: Some(hint.message_es.clone()),
    };

    match repo.record_failure(&record).await {
        Ok(count) => {
            tracing::info!(
            site = %site_base_url,
            selector = %selector_signature,
            score = hint.score.total,
            failure_count = count,
            "extraction_fingerprint_recorded"
            );
            let mut recorded = record;
            recorded.failure_count = count;
            hint.fingerprint = Some(recorded);
        },
        Err(e) => {
            tracing::warn!(
            error = %e,
            site = %site_base_url,
            "extraction fingerprint recording failed — continuing without it"
            );
        },
    }
}
