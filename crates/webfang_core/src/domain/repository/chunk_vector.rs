//! Chunk embedding value object and the composite repository that reads it.
//!
//! [`ChunkVector`] is the domain value object returned by bulk chunk-vector
//! reads. [`MultiVectorRepository`] fans those reads — and every write — out to
//! all inner sinks, so no vector destination is silently dropped.

use std::future::Future;
use std::pin::Pin;

use crate::domain::embedding_port::cosine_similarity_fail_closed;
use crate::domain::text_chunker::ByteSpan;
use crate::error::ScraperError;

use super::DynVectorRepository;
use super::VectorRepository;

/// A stored chunk embedding, ready for in-memory cosine ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkVector {
    /// Chunk row id from the general `chunks` table.
    pub id: String,
    /// The chunk's text content.
    pub content: String,
    /// The decoded little-endian `f32` embedding.
    pub embedding: Vec<f32>,
}

impl ChunkVector {
    /// Score this chunk against a query with the same fail-closed cosine helper
    /// used to rank `note_chunks`.
    ///
    /// Returns `None` on a dimension mismatch: the chunk must be skipped, never
    /// ranked as a zero score.
    #[must_use]
    pub fn cosine_score(&self, query: &[f32]) -> Option<f32> {
        cosine_similarity_fail_closed(query, &self.embedding)
    }
}

/// Composite [`VectorRepository`] that fans out every call to a set of inner
/// repositories, so the elastic pipeline can persist to SQLite (`--elastic`)
/// **and** emit JSONL (`--output-vectors`) simultaneously (issue #636).
///
/// Fan-out pattern: every write is attempted on all inner sinks, an
/// individual failure is logged but the remaining sinks still run, and the
/// composite reports success if *at least one* sink succeeded. Read-style
/// lookups (`resource_exists_by_hash`,
/// `get_vector`) return the first `Some` produced by any inner repository.
///
/// This is what turns `--elastic` and `--output-vectors` from mutually
/// exclusive `if/else if` branches into orthogonal data destinations — the
/// single `ElasticIngestion` is built over one of these fan-outs, so no vector
/// output is silently dropped.
pub struct MultiVectorRepository {
    repos: Vec<DynVectorRepository>,
}

impl MultiVectorRepository {
    /// Build a fan-out repository over the given inner sinks.
    ///
    /// An empty set is not an invariant violation here — it simply reports an
    /// error on every write. The
    /// [`Container`](crate::application::container::Container) never builds an
    /// empty fan-out (it returns early when no sink is active).
    pub fn new(repos: Vec<DynVectorRepository>) -> Self {
        Self { repos }
    }
}

impl VectorRepository for MultiVectorRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            if self.repos.is_empty() {
                return Err(ScraperError::persistence(
                    "no hay sinks de vectores registrados",
                ));
            }
            let mut first_url: Option<String> = None;
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo
                    .save_resource(url, title, content_hash, size_bytes)
                    .await
                {
                    Ok(u) => {
                        if first_url.is_none() {
                            first_url = Some(u);
                        }
                    },
                    Err(e) => {
                        tracing::error!(url = %url, error = %e, "vector sink save_resource failed");
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_url {
                Some(u) => Ok(u),
                None => Err(first_err.unwrap_or_else(|| {
                    ScraperError::persistence(
                        "todos los sinks de vectores fallaron al guardar el recurso",
                    )
                })),
            }
        })
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
        Box::pin(async move {
            if self.repos.is_empty() {
                return Err(ScraperError::persistence(
                    "no hay sinks de vectores registrados",
                ));
            }
            let mut any_ok = false;
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo
                    .save_chunk(id, resource_url, chunk_index, content, embedding, spans)
                    .await
                {
                    Ok(()) => any_ok = true,
                    Err(e) => {
                        tracing::error!(
                            url = %resource_url,
                            chunk_id = %id,
                            chunk_index,
                            error = %e,
                            "vector sink save_chunk failed"
                        );
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            if any_ok {
                Ok(())
            } else {
                Err(first_err.unwrap_or_else(|| {
                    ScraperError::persistence(
                        "todos los sinks de vectores fallaron al guardar el chunk",
                    )
                }))
            }
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo.resource_exists_by_hash(content_hash).await {
                    Ok(Some(url)) => return Ok(Some(url)),
                    Ok(None) => {},
                    Err(e) => {
                        tracing::error!(
                            content_hash = %content_hash,
                            error = %e,
                            "vector sink resource_exists_by_hash failed"
                        );
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(None),
            }
        })
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo.get_vector(chunk_id).await {
                    Ok(Some(v)) => return Ok(Some(v)),
                    Ok(None) => {},
                    Err(e) => {
                        tracing::error!(
                            chunk_id = %chunk_id,
                            error = %e,
                            "vector sink get_vector failed"
                        );
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(None),
            }
        })
    }

    fn load_chunk_vectors<'a>(
        &'a self,
        resource_url: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChunkVector>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo.load_chunk_vectors(resource_url).await {
                    Ok(rows) if !rows.is_empty() => return Ok(rows),
                    Ok(_) => {},
                    Err(e) => {
                        tracing::error!(
                            resource_url = ?resource_url,
                            error = %e,
                            "vector sink load_chunk_vectors failed"
                        );
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(Vec::new()),
            }
        })
    }
}
