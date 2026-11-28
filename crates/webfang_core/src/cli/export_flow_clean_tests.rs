use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::cli::error::CliExit;
use crate::domain::semantic_cleaner::{private::Sealed, SemanticCleaner};
use crate::domain::value_objects::ValidUrl;
use crate::domain::DocumentChunk;
use crate::domain::ScrapedContent;
use crate::error::SemanticError;

use super::clean_all_pages;

/// Mock cleaner that always fails — used to assert total-failure propagation.
struct FailingCleaner;

impl Sealed for FailingCleaner {}

impl SemanticCleaner for FailingCleaner {
    fn clean<'a>(
        &'a self,
        _url: &'a str,
        _html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            Err(SemanticError::Inference(
                "mock cleaner always fails (#543 regression)".into(),
            ))
        })
    }

    fn max_chars(&self) -> usize {
        512
    }

    fn is_ready(&self) -> bool {
        true
    }
}

fn sample_result(url: &str, html: &str) -> ScrapedContent {
    ScrapedContent {
        title: String::new(),
        content: String::new(),
        url: ValidUrl::parse(url).expect("valid test url"),
        excerpt: None,
        author: None,
        date: None,
        html: Some(html.to_string()),
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    }
}

/// #543 regression: when EVERY page fails to clean, the pipeline must return
/// an error (non-zero exit) instead of silently exiting 0 with raw fallback.
///
/// With the ErrorClass fail-fast design (#581 follow-up), an infrastructure-
/// fatal error (Inference/ModelLoad) aborts the whole crawl on the FIRST
/// page instead of processing all pages just to discover they all fail.
#[tokio::test]
async fn test_clean_all_pages_total_failure_propagates_error() {
    let cleaner: Arc<dyn SemanticCleaner> = Arc::new(FailingCleaner);
    let results = vec![
        sample_result("https://example.com/a", "<p>a</p>"),
        sample_result("https://example.com/b", "<p>b</p>"),
    ];

    let outcome = clean_all_pages(&results, &cleaner, None, false).await;

    // Inference error is InternalFatal → fail-fast: abort immediately with
    // a config error instead of waiting for all pages to fail.
    match outcome {
        Err(CliExit::ConfigError(msg)) => {
            assert!(
                msg.contains("fatal") || msg.contains("infraestructura"),
                "error should indicate infrastructure failure: {msg}"
            );
        },
        other => panic!(
            "expected Err(CliExit::ConfigError) for infrastructure failure, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
}

/// Build the chunk a healthy page would produce, with a non-empty body.
fn cleaned_chunk(url: &str, html: &str) -> DocumentChunk {
    let mut page = sample_result(url, html);
    page.content = html.to_string();
    DocumentChunk::from_scraped_content(&page)
}

/// Cleaner whose per-page behavior is scripted by the test: URLs carrying
/// `slow` never resolve (a remote embedding call still in flight when the
/// signal arrives), every other page returns one chunk immediately.
struct ScriptedCleaner {
    hang_slow: bool,
}

impl Sealed for ScriptedCleaner {}

impl SemanticCleaner for ScriptedCleaner {
    fn clean<'a>(
        &'a self,
        url: &'a str,
        html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>> {
        let hang = self.hang_slow && url.contains("slow");
        Box::pin(async move {
            if hang {
                std::future::pending::<()>().await;
            }
            Ok(vec![cleaned_chunk(url, html)])
        })
    }

    fn max_chars(&self) -> usize {
        512
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// #2270: a cancelled token stops the cleaning at the NEXT page and keeps
/// the work that already finished. The page still in flight is dropped
/// instead of awaited, so the run cannot keep paying for embeddings the
/// user already asked it to stop requesting.
#[tokio::test]
async fn test_clean_all_pages_stops_at_the_next_page_when_cancelled() {
    let cleaner: Arc<dyn SemanticCleaner> = Arc::new(ScriptedCleaner { hang_slow: true });
    let results = vec![
        sample_result("https://example.com/fast", "<p>fast</p>"),
        sample_result("https://example.com/slow", "<p>slow</p>"),
    ];
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();

    let outcome = clean_all_pages(&results, &cleaner, Some(&token), false)
        .await
        .expect("a cancelled cleaning is not an error");

    assert_eq!(
        outcome.len(),
        1,
        "only the page that already finished may survive the drain"
    );
    assert!(
        outcome[0].url.ends_with("/fast"),
        "the surviving chunk must be the completed page, got {}",
        outcome[0].url
    );
}

/// Regression guard for the stream rewrite: WITHOUT a token every page is
/// still cleaned and folded — cancellation must not swallow work.
#[tokio::test]
async fn test_clean_all_pages_without_token_cleans_every_page() {
    let cleaner: Arc<dyn SemanticCleaner> = Arc::new(ScriptedCleaner { hang_slow: false });
    let results = vec![
        sample_result("https://example.com/one", "<p>one</p>"),
        sample_result("https://example.com/two", "<p>two</p>"),
    ];

    let outcome = clean_all_pages(&results, &cleaner, None, false)
        .await
        .expect("an uncancelled cleaning succeeds");

    assert_eq!(outcome.len(), 2, "every page must be folded exactly once");
}
