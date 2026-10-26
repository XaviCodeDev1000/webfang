//! Port traits — Domain-level abstractions for infrastructure services.
//!
//! Following Hexagonal Architecture: ports define what the application needs,
//! adapters provide the real implementations. The domain layer owns these
//! trait definitions with ZERO infrastructure dependencies.
//!
//! # Port Types
//!
//! - [`HttpClientPort`] — HTTP fetching abstraction (owned by the domain layer)
//! - [`AssetDownloaderPort`] — asset download abstraction
//! - [`BinaryWriterPort`] — binary payload persistence abstraction
//! - [`ProgressObserver`] — progress event reporting abstraction
//!
//! The `HttpClientPort` trait and its `HttpResponse` DTO are defined in
//! `domain::http_port` (the domain layer owns the contract). The production
//! `wreq`-backed impl and mock tests live in `application::http_client`.
//! We re-export the trait here so downstream code can import from the
//! domain layer without reaching into the application layer.

pub use crate::domain::downloader_port::{Cookie, DownloadError, Downloader, FetchedPage};
pub use crate::domain::http_port::HttpClientPort;

use std::future::Future;
use std::pin::Pin;

use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::value_objects::ValidUrl;

/// Port trait for downloading assets (images, documents).
///
/// Abstracts asset downloading so application services don't depend on
/// specific download implementations. The production adapter streams
/// to disk with ~8KB RAM; tests inject mocks that count calls.
pub trait AssetDownloaderPort: Send + Sync {
    /// Download a batch of assets from validated URLs.
    ///
    /// Returns partial results — individual failures don't abort the batch.
    /// The element type is the domain `ValidUrl` newtype (#1117): a
    /// non-fetchable scheme or embedded credentials cannot even be named
    /// here — the parse lives once at the caller's edge.
    fn download_batch(
        &self,
        urls: &[ValidUrl],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<Vec<crate::domain::entities::DownloadedAsset>>,
                > + Send
                + '_,
        >,
    >;
}

/// Port trait for writing binary payloads (PDFs, images, archives) to disk.
///
/// Abstracts filesystem persistence so application use cases never call
/// `std::fs` directly — Clean Architecture keeps the application layer free of
/// filesystem specifics (issue #442). The production adapter
/// [`FsBinaryWriter`](crate::infrastructure::crawler::FsBinaryWriter) creates
/// the parent directory tree and writes the bytes; tests inject a temp-dir or
/// in-memory implementation.
pub trait BinaryWriterPort: Send + Sync {
    /// Write `bytes` to `path`, creating parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`](crate::error::ScraperError::Io) when parent
    /// directory creation or the write itself fails.
    fn write_bytes(&self, path: &std::path::Path, bytes: &[u8]) -> crate::error::Result<()>;
}

/// Port trait for real-time progress reporting during scraping.
///
/// Implementations receive structured progress events as the scraper
/// processes each URL. The trait is `Send + Sync` so observers can be
/// shared across async tasks.
pub trait ProgressObserver: Send + Sync {
    /// Called when scraping starts for a URL.
    fn on_page_started<'a>(&'a self, url: &'a str)
        -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when the status changes for a URL (Fetching, Extracting, etc.).
    fn on_status_changed<'a>(
        &'a self,
        url: &'a str,
        status: ScrapeStatus,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when a URL is successfully scraped.
    fn on_page_completed<'a>(
        &'a self,
        url: &'a str,
        chars: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when scraping a URL fails.
    fn on_page_failed<'a>(
        &'a self,
        url: &'a str,
        error: &'a ScrapeError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when a URL is blocked by robots.txt.
    fn on_robots_blocked<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called after all URLs have been processed.
    fn on_finished<'a>(
        &'a self,
        total: usize,
        successful: usize,
        failed: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_error::HttpError;
    use crate::domain::http_port::HttpResponse;
    use crate::test_fixtures::MockHttpClient;
    use std::collections::HashMap;

    // --- Test: HttpClientPort trait is object-safe ---

    #[tokio::test]
    async fn test_http_client_port_object_safe() {
        let mock: Box<dyn HttpClientPort> = Box::new(MockHttpClient::new().with_response(
            "https://example.com",
            Ok(HttpResponse {
                status: 200,
                body: "<p>Hello</p>".into(),
                headers: HashMap::new(),
            }),
        ));

        let resp = mock.get("https://example.com").await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "<p>Hello</p>");
    }

    #[tokio::test]
    async fn test_http_client_port_error_propagation() {
        let mock: Box<dyn HttpClientPort> = Box::new(
            MockHttpClient::new()
                .with_response("https://fail.com", Err(HttpError::ClientError(500))),
        );

        let result = mock.get("https://fail.com").await;
        assert!(result.is_err());
    }
}
