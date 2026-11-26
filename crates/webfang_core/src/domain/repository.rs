//! Domain trait for vector persistence (dependency inversion).
//!
//! Defines the persistence contract used by the elastic ingestion pipeline.
//! The infrastructure layer
//! ([`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`])
//! implements this trait; the application layer depends on the trait — not the
//! concrete repo — so SQLite can be swapped or mocked without touching
//! orchestration logic.
//!
//! # Dyn-compatibility (A1 desugar — core-slimming)
//!
//! The four trait methods use manual `async fn` desugaring to
//! `Pin<Box<dyn Future<Output = …> + Send + '_>>` (BoxFuture) instead of native
//! `async fn` in traits. This makes the trait **dyn-compatible** so
//! `Arc<dyn VectorRepository + Send + Sync>` can be used for runtime dispatch
//! (spec S3.4), without adding the `async_trait` crate (frozen decision #1).
//! A blanket impl for `Arc<T>` lets `ElasticIngestion<R: VectorRepository>`
//! accept `Arc<dyn VectorRepository + Send + Sync>` as `R`.

use std::future::Future;
use std::pin::Pin;

use crate::domain::text_chunker::ByteSpan;
use crate::error::ScraperError;

mod chunk_vector;

pub use chunk_vector::{ChunkVector, MultiVectorRepository};

/// Domain trait for vector persistence (dependency inversion).
///
/// Implementations store crawl resources (with content-hash deduplication) and
/// semantic chunks whose embeddings are serialized as raw little-endian `f32`
/// BLOBs (frozen design decision #7).
///
/// All database failures surface as [`ScraperError::Persistence`] (frozen
/// decision #4: no separate `StorageError` enum), matching the pattern
/// established by PR1.
///
/// The methods are desugared to `Pin<Box<dyn Future<…> + Send + '_>>` so the
/// trait is dyn-compatible (A1, spec S3.4) without the `async_trait` crate.
pub trait VectorRepository: Send + Sync {
    /// Save a resource with its content hash. Returns the resource URL.
    ///
    /// If a resource with the same `content_hash` already exists, this
    /// short-circuits (dedup, frozen decision #3) and returns the **existing**
    /// URL without inserting a duplicate row. The same URL with a DIFFERENT
    /// hash is updated in place (#2663): its row holds the most recent content.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>>;

    /// Save a chunk, optionally with its embedding vector and locator spans.
    ///
    /// When `embedding` is `Some`, it is serialized to a little-endian `f32`
    /// BLOB; when `None`, the `embedding_vector` column is stored as SQL `NULL`.
    ///
    /// `spans` (#2429 AC2) are the byte ranges the chunk was cut from inside
    /// the text the chunker segmented. Repositories that do not store
    /// locators accept and ignore them — dropping them on the floor is the
    /// caller's documented contract, never a silent data corruption.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure (e.g. a
    /// foreign-key violation if `resource_url` was never saved first).
    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
        spans: Option<&'a [ByteSpan]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>>;

    /// Check whether a resource with this `content_hash` already exists.
    ///
    /// Returns `Ok(Some(url))` with the existing resource's URL if found, or
    /// `Ok(None)` if no resource has that hash.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>>;

    /// Get the embedding vector for a chunk.
    ///
    /// Returns `Ok(Some(vec))` if the chunk exists and has an embedding,
    /// `Ok(None)` if the chunk is missing or has a `NULL` embedding, or an
    /// error if the stored BLOB is corrupt (length not a multiple of 4).
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on a corrupt BLOB or database
    /// failure.
    #[allow(clippy::type_complexity)]
    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>>;

    /// Load every chunk that has an embedding from the general `chunks` table.
    ///
    /// When `resource_url` is `Some`, only that resource's chunks are returned.
    /// Rows with a `NULL` embedding are excluded, and no matching rows is a
    /// successful empty vector. Write-only sinks use the default and contribute
    /// no readable chunks.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on a database failure or a corrupt
    /// embedding BLOB.
    fn load_chunk_vectors<'a>(
        &'a self,
        _resource_url: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChunkVector>, ScraperError>> + Send + 'a>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Blanket impl so `Arc<dyn VectorRepository + Send + Sync>` satisfies
/// `R: VectorRepository + Send + Sync` in
/// [`crate::application::elastic_ingestion::ElasticIngestion<R>`] (spec S3.4).
///
/// Delegates each method through the `Arc` deref to the inner repository. This
/// is the bridge that lets the Container store
/// `Option<Arc<ElasticIngestion<Arc<dyn VectorRepository + Send + Sync>>>>`
/// for runtime repo dispatch (SQLite when `persistence` is ON, StreamRepository
/// when OFF).
impl<T: VectorRepository + ?Sized> VectorRepository for std::sync::Arc<T> {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        (**self).save_resource(url, title, content_hash, size_bytes)
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
        spans: Option<&'a [ByteSpan]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        (**self).save_chunk(id, resource_url, chunk_index, content, embedding, spans)
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        (**self).resource_exists_by_hash(content_hash)
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        (**self).get_vector(chunk_id)
    }

    fn load_chunk_vectors<'a>(
        &'a self,
        resource_url: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChunkVector>, ScraperError>> + Send + 'a>> {
        (**self).load_chunk_vectors(resource_url)
    }
}

/// Erased vector repository for runtime dispatch.
///
/// Lets the Container hold either the SQLite-backed [`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`]
/// (under the `persistence` feature) or the dependency-free `StreamRepository`
/// JSONL sink behind a single type, enabling `Arc<dyn VectorRepository + Send + Sync>`
/// to satisfy the `R: VectorRepository + Send + Sync` bound on
/// [`crate::application::elastic_ingestion::ElasticIngestion`].
pub type DynVectorRepository = std::sync::Arc<dyn VectorRepository + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Minimal in-memory sink for asserting fan-out behavior and failures.
    struct InMemoryRepo {
        saves: Arc<AtomicUsize>,
        chunks: Arc<AtomicUsize>,
        fail_write: bool,
    }

    impl InMemoryRepo {
        fn new(saves: Arc<AtomicUsize>, chunks: Arc<AtomicUsize>, fail_write: bool) -> Self {
            Self {
                saves,
                chunks,
                fail_write,
            }
        }
    }

    impl VectorRepository for InMemoryRepo {
        fn save_resource<'a>(
            &'a self,
            url: &'a str,
            _title: &'a str,
            _content_hash: &'a str,
            _size_bytes: u64,
        ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                self.saves.fetch_add(1, Ordering::SeqCst);
                if self.fail_write {
                    Err(ScraperError::persistence("mock save_resource failure"))
                } else {
                    Ok(url.to_string())
                }
            })
        }

        fn save_chunk<'a>(
            &'a self,
            _id: &'a str,
            _resource_url: &'a str,
            _chunk_index: i64,
            _content: &'a str,
            _embedding: Option<&'a [f32]>,
            _spans: Option<&'a [ByteSpan]>,
        ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                self.chunks.fetch_add(1, Ordering::SeqCst);
                if self.fail_write {
                    Err(ScraperError::persistence("mock save_chunk failure"))
                } else {
                    Ok(())
                }
            })
        }

        fn resource_exists_by_hash<'a>(
            &'a self,
            _content_hash: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }

        fn get_vector<'a>(
            &'a self,
            _chunk_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }
    }

    fn repo(
        saves: &Arc<AtomicUsize>,
        chunks: &Arc<AtomicUsize>,
        fail_write: bool,
    ) -> DynVectorRepository {
        Arc::new(InMemoryRepo::new(saves.clone(), chunks.clone(), fail_write))
    }

    #[tokio::test]
    async fn fan_out_writes_to_all_inner_sinks() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![repo(&s1, &c1, false), repo(&s2, &c2, false)]);

        multi.save_resource("u", "t", "hash", 1).await.unwrap();
        multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]), None)
            .await
            .unwrap();

        assert_eq!(s1.load(Ordering::SeqCst), 1);
        assert_eq!(s2.load(Ordering::SeqCst), 1);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn continues_on_single_sink_failure() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![
            repo(&s1, &c1, true),  // fails
            repo(&s2, &c2, false), // ok
        ]);

        assert!(multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]), None)
            .await
            .is_ok());
        assert!(multi.save_resource("u", "t", "h", 1).await.is_ok());
        // Best-effort: the failing sink was still attempted.
        assert_eq!(s1.load(Ordering::SeqCst), 1);
        assert_eq!(s2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn all_sinks_failing_returns_error() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![repo(&s1, &c1, true), repo(&s2, &c2, true)]);

        let err = multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ScraperError::Persistence(_)));
    }

    #[tokio::test]
    async fn empty_fanout_reports_error() {
        let multi = MultiVectorRepository::new(vec![]);
        let err = multi.save_resource("u", "t", "h", 1).await.unwrap_err();
        assert!(matches!(err, ScraperError::Persistence(_)));
    }
}
