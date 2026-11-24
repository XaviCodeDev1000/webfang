use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::extraction_quality::{
    ExtractionQualityHint, FingerprintRecord, StructuralScore,
};
use crate::domain::fingerprint_repository::FingerprintRepository;
use crate::domain::{ScrapedContent, ValidUrl};
use crate::error::ScraperError;
use url::Url;

/// Mock repository capturing every recorded fingerprint.
#[derive(Default)]
struct CapturingRepo {
    recorded: Mutex<Vec<FingerprintRecord>>,
    next_count: Mutex<u32>,
}

impl FingerprintRepository for CapturingRepo {
    fn record_failure<'a>(
        &'a self,
        record: &'a FingerprintRecord,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        let record = record.clone();
        Box::pin(async move {
            self.recorded.lock().unwrap().push(record);
            let mut count = self.next_count.lock().unwrap();
            *count += 1;
            Ok(*count)
        })
    }

    fn get_failure_count<'a>(
        &'a self,
        _site: &'a str,
        _signature: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(0) })
    }
}

fn hint_with_score(total: f64) -> ExtractionQualityHint {
    ExtractionQualityHint {
        score: StructuralScore {
            semantic_drift: 0.3,
            context_collapse: 0.3,
            result_size: 0.3,
            total,
            active_factors: 3,
        },
        message_es: format!("low-quality ({total}/100)"),
        fingerprint: None,
    }
}

fn scraped_with_hint(hint: ExtractionQualityHint) -> ScrapedContent {
    let url = Url::parse("https://example.com/article").unwrap();
    ScrapedContent {
        title: "t".into(),
        content: "c".into(),
        url: ValidUrl::new(url),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: Some(hint),
    }
}

/// A hinted extraction is recorded and the count attaches to the hint.
#[tokio::test]
async fn hinted_extraction_records_fingerprint_and_attaches_count() {
    let repo = CapturingRepo::default();
    let url = Url::parse("https://example.com/article").unwrap();
    let mut content = scraped_with_hint(hint_with_score(35.0));

    super::record_extraction_fingerprint(&url, Some(&repo), "article|.body", &mut content).await;

    let recorded = repo.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1, "hinted extraction must be recorded");
    assert_eq!(recorded[0].site_base_url, "https://example.com");
    assert_eq!(recorded[0].selector_signature, "article|.body");
    assert_eq!(recorded[0].score_at_failure, 35.0);

    let hint = content.quality_hint.as_ref().expect("hint must survive");
    let fp = hint
        .fingerprint
        .as_ref()
        .expect("count must attach to hint");
    assert_eq!(fp.failure_count, 1);
}

/// A clean extraction (no hint) records nothing.
#[tokio::test]
async fn clean_extraction_records_nothing() {
    let repo = CapturingRepo::default();
    let url = Url::parse("https://example.com/article").unwrap();
    let mut content = scraped_with_hint(hint_with_score(35.0));
    content.quality_hint = None;

    super::record_extraction_fingerprint(&url, Some(&repo), "article|.body", &mut content).await;

    assert!(repo.recorded.lock().unwrap().is_empty());
}

/// No repository wired (flag off) → no-op, hint untouched.
#[tokio::test]
async fn missing_repo_is_a_noop() {
    let url = Url::parse("https://example.com/article").unwrap();
    let mut content = scraped_with_hint(hint_with_score(35.0));

    super::record_extraction_fingerprint(&url, None, "article|.body", &mut content).await;

    assert!(
        content.quality_hint.as_ref().unwrap().fingerprint.is_none(),
        "no repo → no fingerprint attached"
    );
}

/// Flag off → no repository is built.
#[tokio::test]
async fn flag_off_builds_no_repo() {
    let opts = CrawlOptions::default();
    assert!(!opts.extraction_fingerprint);
    assert!(super::build_fingerprint_repo(&opts).await.is_none());
}

/// Flag on → a repository is always produced (SQLite or degraded no-op).
#[tokio::test]
async fn flag_on_builds_a_repo() {
    // Point the DB at a temp dir so the test never touches ~/.webfang.
    let tmp = tempfile::TempDir::new().unwrap();
    let opts = CrawlOptions {
        extraction_fingerprint: true,
        elastic: crate::application::crawl_options::IngestionTuning {
            db_path: Some(tmp.path().join("fp.db")),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(super::build_fingerprint_repo(&opts).await.is_some());
}
