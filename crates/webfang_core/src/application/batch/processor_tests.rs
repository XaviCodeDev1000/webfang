use super::*;
use crate::application::batch::{BatchJob, BatchJobStatus, BatchProgress};
use crate::domain::CrawlerConfig;
use url::Url;

#[test]
fn test_batch_processor_creation() {
    let processor = BatchProcessor::new(5).unwrap();
    assert_eq!(processor.max_concurrent(), 5);
}

#[test]
fn test_batch_processor_zero_concurrency_returns_error() {
    let result = BatchProcessor::new(0);
    assert!(result.is_err(), "zero concurrency should return Err");
    let err = result.err().unwrap();
    assert!(
        matches!(err, BatchError::InvalidConcurrency),
        "expected InvalidConcurrency, got: {err}"
    );
}

#[tokio::test]
async fn cancelled_batch_dispatches_no_urls() {
    // Regression for #653: a shutdown signal must stop the batch from
    // dispatching further URLs instead of running to completion.
    let processor = BatchProcessor::new(2).unwrap();
    let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let job = BatchJob::new(
        "cancelled".to_string(),
        (0..8).map(|i| format!("https://example.com/{i}")).collect(),
        config,
    );

    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = processor
        .process_batch_cancellable(job, &cancel)
        .await
        .expect("a cancelled batch still reports its (empty) result");

    assert_eq!(result.succeeded, 0, "no URL may be crawled after shutdown");
    assert_eq!(result.failed, 0, "skipped URLs are not failures");
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn test_process_empty_batch() {
    let processor = BatchProcessor::new(3).unwrap();
    let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let job = BatchJob::new("test-1".to_string(), vec![], config);

    let result = processor.process_batch(job).await;
    assert!(matches!(result, Err(BatchError::EmptyBatch)));
}

#[tokio::test]
async fn test_batch_progress_concurrent_updates() {
    let progress = BatchProgress::new(100);
    let mut join_set = JoinSet::new();

    // Deterministic outcomes: first 50 succeed, last 50 fail
    for i in 0..100 {
        let p = progress.clone();
        join_set.spawn(async move {
            p.start_one();
            tokio::task::yield_now().await;
            if i < 50 {
                p.complete_one();
                true
            } else {
                p.fail_one();
                false
            }
        });
    }

    let mut successes = 0;
    let mut failures = 0;
    while let Some(result) = join_set.join_next().await {
        if result.unwrap() {
            successes += 1;
        } else {
            failures += 1;
        }
    }

    assert_eq!(successes + failures, 100);
    assert_eq!(progress.completed(), 50);
    assert_eq!(progress.failed(), 50);
    assert!(progress.is_complete());
}

#[test]
fn test_batch_result_display() {
    let result = BatchResult {
        job_id: "test-1".to_string(),
        total: 10,
        succeeded: 8,
        failed: 2,
        errors: vec![
            (
                "https://example.com/404".to_string(),
                ScraperError::http(404, "https://example.com/404"),
            ),
            (
                "https://example.com/timeout".to_string(),
                ScraperError::Network(Box::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Timeout",
                ))),
            ),
        ],
        resolution_failures: 0,
        block_failures: 0,
        aborted_on_resolution: false,
    };

    assert_eq!(result.total, 10);
    assert_eq!(result.succeeded, 8);
    assert_eq!(result.failed, 2);
    assert_eq!(result.errors.len(), 2);
}

#[test]
fn test_batch_progress_clone() {
    let progress = BatchProgress::new(5);
    progress.complete_one();
    progress.complete_one();

    let cloned = progress.clone();
    assert_eq!(cloned.total(), 5);
    assert_eq!(cloned.completed(), 2);
}

// =====================================================================
// BatchResult structure tests
// =====================================================================

#[test]
fn test_batch_result_all_succeeded() {
    let result = BatchResult {
        job_id: "job-ok".to_string(),
        total: 3,
        succeeded: 3,
        failed: 0,
        errors: vec![],
        resolution_failures: 0,
        block_failures: 0,
        aborted_on_resolution: false,
    };
    assert_eq!(result.total, result.succeeded);
    assert_eq!(result.failed, 0);
    assert!(result.errors.is_empty());
}

#[test]
fn test_batch_result_all_failed() {
    let result = BatchResult {
        job_id: "job-fail".to_string(),
        total: 2,
        succeeded: 0,
        failed: 2,
        errors: vec![
            (
                "https://a.com".to_string(),
                ScraperError::Internal("error a".to_string()),
            ),
            (
                "https://b.com".to_string(),
                ScraperError::Internal("error b".to_string()),
            ),
        ],
        resolution_failures: 0,
        block_failures: 0,
        aborted_on_resolution: false,
    };
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, result.total);
    assert_eq!(result.errors.len(), 2);
}

#[test]
fn test_batch_result_counts_consistent() {
    let errors: Vec<(String, ScraperError)> = (0..5)
        .map(|i| {
            (
                format!("url-{i}"),
                ScraperError::Internal(format!("err-{i}")),
            )
        })
        .collect();
    let result = BatchResult {
        job_id: "job-mixed".to_string(),
        total: 10,
        succeeded: 5,
        failed: 5,
        errors,
        resolution_failures: 0,
        block_failures: 0,
        aborted_on_resolution: false,
    };
    assert_eq!(result.succeeded + result.failed, result.total);
    assert_eq!(result.errors.len(), result.failed);
}

// =====================================================================
// BatchProcessor concurrency tests
// =====================================================================

#[test]
fn test_batch_processor_various_concurrency_values() {
    for n in [1, 2, 4, 8, 16] {
        let processor = BatchProcessor::new(n).unwrap();
        assert_eq!(processor.max_concurrent(), n);
    }
}

#[test]
fn test_batch_processor_single_concurrency() {
    let processor = BatchProcessor::new(1).unwrap();
    assert_eq!(processor.max_concurrent(), 1);
}

// =====================================================================
// BatchJob status transitions
// =====================================================================

#[test]
fn test_batch_job_lifecycle() {
    let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let mut job = BatchJob::new(
        "lifecycle".to_string(),
        vec!["https://example.com".to_string()],
        config,
    );

    assert_eq!(job.status, BatchJobStatus::Pending);

    job.start();
    assert_eq!(job.status, BatchJobStatus::Running);

    job.complete();
    assert_eq!(job.status, BatchJobStatus::Completed);
}

#[test]
fn test_batch_job_failure_state() {
    let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let mut job = BatchJob::new("fail-job".to_string(), vec![], config);

    job.fail("network error".to_string());
    assert_eq!(
        job.status,
        BatchJobStatus::Failed("network error".to_string())
    );
}

#[test]
fn test_batch_job_status_display() {
    assert_eq!(BatchJobStatus::Pending.to_string(), "Pending");
    assert_eq!(BatchJobStatus::Running.to_string(), "Running");
    assert_eq!(BatchJobStatus::Completed.to_string(), "Completed");
    assert_eq!(
        BatchJobStatus::Failed("oops".to_string()).to_string(),
        "Failed: oops"
    );
}

// =====================================================================
// BatchProgress edge cases
// =====================================================================

#[test]
fn test_batch_progress_percent_partial() {
    let progress = BatchProgress::new(4);
    progress.start_one();
    progress.complete_one();
    progress.start_one();
    progress.fail_one();

    assert!((progress.percent() - 50.0).abs() < f64::EPSILON);
}

#[test]
fn test_batch_progress_in_progress_count() {
    let progress = BatchProgress::new(5);
    assert_eq!(progress.in_progress(), 0);

    progress.start_one();
    assert_eq!(progress.in_progress(), 1);

    progress.start_one();
    assert_eq!(progress.in_progress(), 2);

    progress.complete_one();
    assert_eq!(progress.in_progress(), 1);

    progress.fail_one();
    assert_eq!(progress.in_progress(), 0);
}

// =====================================================================
// BatchError display tests
// =====================================================================

#[test]
fn test_batch_error_empty_batch_display() {
    let err = BatchError::EmptyBatch;
    assert!(err.to_string().contains("no URLs"));
}

#[test]
fn test_batch_error_crawl_failed_display() {
    let err = BatchError::CrawlFailed {
        url: "https://example.com".to_string(),
        error: CrawlError::InvalidUrl("bad url".to_string()),
    };
    let msg = err.to_string();
    assert!(msg.contains("example.com"));
    assert!(msg.contains("crawl failed"));
}

#[test]
fn test_batch_error_semaphore_closed_display() {
    let err = BatchError::SemaphoreClosed;
    assert!(err.to_string().contains("semaphore"));
}
