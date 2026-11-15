//! Download pipeline of the asset [`Downloader`]: `download` (dedup fast/slow
//! path), `run_download`, retry loop, single attempt, batch, and filename
//! generation. Implemented as `impl Downloader` in this child module.
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use futures::stream::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::domain::ValidUrl;
use crate::error::{ErrorClass, Result, ScraperError};

use super::cache::InFlightGuard;
use super::naming::{
    compute_backoff_delay, derive_slug_from_url, into_stream, mime_type_to_extension,
    parse_content_disposition_header, sanitize_filename, url_matches_filters,
};
use super::{AssetNamingStrategy, DownloadedAsset, Downloader};

impl Downloader {
    pub(crate) fn ensure_subdir(&self, path: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(path).map_err(ScraperError::Io)
    }

    /// Disambiguate a filename that already exists on disk by appending a short
    /// content-hash suffix (preserving the extension).
    pub(crate) fn resolve_collision(
        &self,
        subdir: &std::path::Path,
        filename: &str,
        content_hash: &str,
    ) -> std::path::PathBuf {
        let hash_suffix = &content_hash[..8];
        let stem = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = std::path::Path::new(filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if ext.is_empty() {
            subdir.join(format!("{stem}-{hash_suffix}"))
        } else {
            subdir.join(format!("{stem}-{hash_suffix}.{ext}"))
        }
    }

    /// Download a single asset with true streaming to disk.
    ///
    /// # Dedup (#782)
    ///
    /// Each canonical URL is fetched at most once per `Downloader` (one
    /// instance is shared via `Arc` across all pages of a crawl). The first
    /// occurrence wins and records the downloaded asset in the crawl-scoped
    /// registry; concurrent callers for the same URL wait on that result
    /// (never a second network request); later occurrences — e.g. another
    /// page that references the same asset — reuse the already-written file
    /// and therefore resolve to the same local path. Failures are NOT
    /// cached: the registry only stores successes, so failed URLs never
    /// poison later pages and every caller keeps the typed error verbatim.
    ///
    /// # Architecture
    ///
    /// - Creates temp file with UUID
    /// - Streams chunks directly to disk (constant RAM)
    /// - Computes hash on-the-fly
    /// - Atomic rename on success
    /// - Cleanup temp file on failure
    /// - Retry with exponential backoff on transient network errors
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Network` if HTTP request fails.
    /// Returns `ScraperError::Io` if file operations fail.
    /// Returns `ScraperError::Download` if file exceeds size limit.
    /// Download one asset. The parameter is the validated [`ValidUrl`]
    /// typestate (#1117): hostile strings (non-http schemes, embedded
    /// credentials) are rejected by the caller's boundary —
    /// [`ValidUrl::parse`] / [`ValidUrl::try_from_url`] — before this
    /// method can hand them to the TLS pool.
    pub async fn download(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let key = url.as_str();
        // Fast path: a previous page already downloaded this asset (#782).
        // The duplicate is SKIPPED here — no network request is sent.
        let registered = self
            .downloaded_urls
            .get(key)
            .map(|entry| std::sync::Arc::clone(entry.value()));
        if let Some(cell) = registered {
            if let Some(asset) = cell.get() {
                tracing::debug!(
                    url = %url,
                    local_path = %asset.local_path.display(),
                    reason = "already_downloaded_this_crawl",
                    "duplicate asset download skipped — reusing cached file"
                );
                return Ok(asset.clone());
            }
            // Cell exists but is uninitialized: download still in flight or a
            // previous failure — fall through and wait/retry via the slow path.
        }

        let cell = self
            .downloaded_urls
            .entry(key.to_string())
            .or_insert_with(|| std::sync::Arc::new(OnceCell::new()))
            .clone();
        // Track insertion order for bounded-cache FIFO eviction. The unbounded
        // legacy path skips the ledger entirely: zero extra allocations, zero
        // mutex traffic, byte-identical behavior.
        if self.asset_cache_capacity != usize::MAX {
            // Same recovery as `register`: a poisoned ledger must not disable
            // eviction for the rest of the run.
            let mut order = self
                .asset_cache_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            order.push_new(key.to_string());
        }
        self.evict_over_capacity();

        // Slow path: exactly one caller for this URL runs the download (with
        // retries); concurrent arrivals wait on the same cell and never open
        // a second connection. Failures leave the cell uninitialized (tokio
        // hands a fresh attempt to the waiters), so typed errors reach every
        // caller and no URL is poisoned for later pages. The RAII guard marks
        // this URL as actively downloading — and clears the marker on
        // completion, failure, or task cancellation (#509) alike — which is
        // what lets eviction distinguish "in flight" from "abandoned".
        let _in_flight = InFlightGuard::new(&self.in_flight_downloads, key);
        cell.get_or_try_init(|| self.run_download(url))
            .await
            .cloned()
    }

    /// Run the retry/backoff download and record the outcome in the crawl
    /// dedup registry (#782). Successes are observable via the emitted
    /// structured event; failures pass through without caching.
    async fn run_download(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        match self.download_with_retry(url).await {
            Ok(asset) => {
                tracing::debug!(
                    url = %url,
                    local_path = %asset.local_path.display(),
                    bytes = asset.size,
                    "asset download recorded in crawl dedup registry"
                );
                Ok(asset)
            },
            Err(e) => {
                tracing::debug!(
                    url = %url,
                    err = %e,
                    "asset download failed — URL left uncached so later pages may retry"
                );
                Err(e)
            },
        }
    }

    /// Retry loop with exponential backoff for a single asset download.
    ///
    /// # Errors
    ///
    /// Returns the first non-transient error, or the last transient error
    /// after exhausting `max_retries`.
    async fn download_with_retry(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let mut last_err = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = compute_backoff_delay(
                    attempt,
                    self.config.backoff_base_ms,
                    self.config.backoff_max_ms,
                );
                tracing::debug!(
                    "retry {attempt}/{} for {} after {}ms",
                    self.config.max_retries,
                    url,
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }

            match self.download_once(url).await {
                Ok(asset) => return Ok(asset),
                Err(e) => {
                    if e.classify() != ErrorClass::TransientRetriable
                        || attempt == self.config.max_retries
                    {
                        return Err(e);
                    }
                    last_err = Some(e);
                },
            }
        }

        // Unreachable: loop always returns on last attempt, but required for type inference
        // LCOV_EXCL_LINE defensive: unreachable-retry-exhaustion — the retry loop always returns on the last attempt
        Err(last_err.unwrap_or_else(|| {
            ScraperError::Internal("exhausted retries with no error captured".to_string())
        }))
    }

    /// Single download attempt (no retry).
    #[tracing::instrument(
        skip(self),
        fields(
            url = %url,
            // D5: stable identity of the shared pooled `Client`. Constant across
            // all asset downloads within a run => observable proof of connection-pool
            // reuse (no silent per-request handshake). See MAPA item 7.
            client_id = %format!("{:p}", &self.client)
        )
    )]
    async fn download_once(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let response = self
            .client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| ScraperError::Network(Box::new(e)))?;

        // Fail-fast on any non-success status. Covers 4xx (client errors),
        // 5xx (transient, retried by `download`) and terminal redirect
        // responses surfaced when the SSRF redirect guard `stop()`s following
        // (#703): without this gate the stopped 301 body would be streamed
        // into an asset file.
        let status = response.status();
        if !status.is_success() {
            return Err(ScraperError::http(status.as_u16(), url.as_str()));
        }

        let mime_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Extract Content-Disposition filename before consuming response
        let content_disposition_filename = response
            .headers()
            .get(wreq::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_header);

        let asset_type = crate::adapters::detector::detect_from_url(url.as_str());
        let subdir = if asset_type.is_image() {
            &self.config.images_dir
        } else {
            &self.config.documents_dir
        };

        let subdir_path = self.config.output_dir.join(subdir);
        // Lazily create the target subdir only when we are about to persist a
        // real asset (issue #606) — not at `Downloader::new` time.
        self.ensure_subdir(&subdir_path)?;

        // Create temp file with UUID (atomic operation pattern)
        let temp_path = subdir_path.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(ScraperError::Io)?;

        // Stream to disk with real-time size check
        let mut stream = into_stream(response);
        let mut downloaded: u64 = 0;
        let mut hasher = Sha256::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| ScraperError::Network(Box::new(e)))?;
            if chunk.is_empty() {
                continue;
            }

            let chunk_len = chunk.len() as u64;
            // LCOV_EXCL_LINE defensive: integer-overflow — u64 overflow requires an unrealistic download size
            downloaded = downloaded.checked_add(chunk_len).ok_or_else(|| {
                ScraperError::Internal("integer overflow in download size".to_string())
            })?;

            // Check limit in real-time
            if downloaded > self.config.max_file_size {
                let _ = fs::remove_file(&temp_path).await;
                return Err(ScraperError::PayloadTooLarge);
            }

            // Write chunk to disk IMMEDIATELY (true streaming)
            file.write_all(&chunk).await.map_err(ScraperError::Io)?;
            hasher.update(&chunk);
        }

        // Sync to ensure data is on disk
        file.sync_all().await.map_err(ScraperError::Io)?;
        drop(file); // Close file before rename

        // Calculate hash and generate final filename
        let content_hash = format!("{:x}", hasher.finalize());
        let filename = self.generate_filename(
            url,
            &content_hash,
            mime_type.as_deref(),
            content_disposition_filename.as_deref(),
        );
        let mut final_path = subdir_path.join(&filename);
        // Slug/ContentDisposition naming can collide when different URLs produce
        // the same filename. Disambiguate with a short hash suffix.
        if final_path.exists() {
            final_path = self.resolve_collision(&subdir_path, &filename, &content_hash);
        }

        // Atomic rename
        fs::rename(&temp_path, &final_path)
            .await
            .map_err(ScraperError::Io)?;

        tracing::info!(
            client_id = %format!("{:p}", &self.client),
            "downloaded: {} -> {:?}",
            url,
            final_path
        );

        Ok(DownloadedAsset {
            url: url.as_str().to_string(),
            local_path: final_path,
            mime_type,
            size: downloaded,
            content_hash: content_hash[..12].to_string(),
        })
    }

    /// Download multiple assets with configurable concurrency control.
    ///
    /// Filters URLs against `include_patterns` / `exclude_patterns` before downloading.
    /// Returns partial results — individual failures don't abort the batch.
    pub async fn download_batch(&self, urls: &[ValidUrl]) -> Vec<Result<DownloadedAsset>> {
        if urls.is_empty() {
            return Vec::new();
        }

        let filtered: Vec<&ValidUrl> = urls
            .iter()
            .filter(|url| {
                url_matches_filters(
                    url,
                    &self.config.include_patterns,
                    &self.config.exclude_patterns,
                )
            })
            .collect();

        if filtered.is_empty() {
            return Vec::new();
        }

        let concurrency = self.config.concurrency_limit;
        let mut results = Vec::with_capacity(filtered.len());
        let mut futs = Vec::with_capacity(filtered.len());
        for url in filtered {
            futs.push(self.download(url));
        }
        let stream = futures::stream::iter(futs).buffer_unordered(concurrency);
        results.extend(stream.collect::<Vec<_>>().await);
        results
    }

    /// Generate filename according to the configured naming strategy.
    pub(crate) fn generate_filename(
        &self,
        url: &ValidUrl,
        content_hash: &str,
        mime_type: Option<&str>,
        content_disposition_filename: Option<&str>,
    ) -> String {
        let extension =
            mime_type_to_extension(mime_type.unwrap_or("")).unwrap_or_else(|| "bin".into());

        let base_name = match self.config.asset_naming {
            AssetNamingStrategy::Hash => {
                // Use first 12 characters of hash (96 bits of entropy)
                format!("{}.{}", &content_hash[..12], extension)
            },
            AssetNamingStrategy::Slug => {
                let slug = derive_slug_from_url(url);
                let name = sanitize_filename(&slug);
                if name.is_empty() {
                    // Fallback to hash if slug is empty
                    format!("{}.{}", &content_hash[..12], extension)
                } else {
                    // Preserve original extension from slug if present, otherwise use MIME
                    let slug_ext = name.rsplit('.').next().unwrap_or("");
                    if !slug_ext.is_empty() && slug_ext != name {
                        sanitize_filename(&name)
                    } else {
                        format!("{name}.{extension}")
                    }
                }
            },
            AssetNamingStrategy::ContentDisposition => {
                if let Some(name) = content_disposition_filename {
                    let sanitized = sanitize_filename(name);
                    if !sanitized.is_empty() {
                        sanitized
                    } else {
                        format!("{}.{}", &content_hash[..12], extension)
                    }
                } else {
                    format!("{}.{}", &content_hash[..12], extension)
                }
            },
        };

        base_name
    }
}
