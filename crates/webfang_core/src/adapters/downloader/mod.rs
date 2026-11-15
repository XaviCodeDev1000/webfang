//! Asset Download Module
//!
//! Handles downloading of images and documents from URLs.
//!
//! # Architecture
//!
//! Following rust-skills best practices:
//! - **True Streaming**: Writes chunks directly to disk, constant RAM (~8KB)
//! - **Atomic Operations**: Temp file with UUID, atomic rename on success
//! - **Init Once**: Directories pre-created in `new()`, zero runtime contention
//! - **Configurable**: User-Agent externalized to config
//! - **Cleanup**: Temp file removed on size limit exceeded
//! - **Hash On-The-Fly**: SHA256 computed during streaming, no buffer needed

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Entries retained per Asset-tier permit in the dedup cache (Q3 MEASURE
/// FIRST: BEFORE data measured ~600 B/entry; default tier 3 x 8_192 =
/// 24_576 entries ~= 15 MiB ceiling, far below the 50 MB materiality line).
pub const ASSET_CACHE_ENTRIES_PER_PERMIT: usize = 8_192;

/// Derive the bounded asset-cache capacity from the budget model's Asset
/// tier. Single source of truth with every other derived bound.
#[must_use]
pub fn asset_cache_capacity(asset_tier_permits: usize) -> usize {
    asset_tier_permits
        .saturating_mul(ASSET_CACHE_ENTRIES_PER_PERMIT)
        .max(ASSET_CACHE_ENTRIES_PER_PERMIT)
}

/// Insertion-order ledger for the bounded dedup cache. A `HashSet` makes
/// membership O(1) so every new cache insert stays O(1) under the single
/// ledger mutex instead of scanning the whole FIFO queue.
#[derive(Default)]
struct InsertionLedger {
    queue: VecDeque<String>,
    seen: HashSet<String>,
}

impl InsertionLedger {
    fn push_new(&mut self, key: String) {
        if self.seen.insert(key.clone()) {
            self.queue.push_back(key);
        }
    }

    /// Rotate an entry (still tracked in `seen`) back to the tail.
    fn requeue(&mut self, key: String) {
        self.queue.push_back(key);
    }

    fn pop_oldest(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    /// Drop membership tracking for an entry that left the cache.
    fn forget(&mut self, key: &str) {
        self.seen.remove(key);
    }

    fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Lifecycle class of a candidate cache cell during eviction.
enum CellState {
    /// Holds a successful `DownloadedAsset`.
    Initialized,
    /// Empty but a download is executing for this URL right now.
    InFlight,
    /// Empty with no active download: a permanent-failure leftover.
    Abandoned,
    /// Key already removed from the map (evicted elsewhere).
    Gone,
}

/// RAII marker for a URL whose download is actively executing. Inserted
/// before `get_or_try_init` and removed on drop — including tokio task
/// cancellation (#509), which drops the future and therefore the guard,
/// so a cancelled download can never leave a stale in-flight marker.
struct InFlightGuard<'a> {
    map: &'a DashMap<String, ()>,
    url: String,
}

impl<'a> InFlightGuard<'a> {
    fn new(map: &'a DashMap<String, ()>, url: &str) -> Self {
        map.insert(url.to_string(), ());
        Self {
            map,
            url: url.to_string(),
        }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.map.remove(&self.url);
    }
}

use crate::domain::ValidUrl;
use crate::error::{ErrorClass, Result, ScraperError};
use dashmap::DashMap;
use futures::stream::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use uuid::Uuid;
use wreq::{Client, Response};
use wreq_util::Profile;

/// Strategy for generating downloaded asset filenames — domain-owned VO.
///
/// Re-exported from `crate::domain::config` so `ScraperConfig` (domain) does
/// not depend on `adapters` (outward). The `adapters` implementation re-uses the
/// domain type via this pub use (inward `adapters → domain` is allowed).
pub use crate::domain::config::AssetNamingStrategy;

/// Result of a successful download
#[derive(Debug, Clone)]
pub struct DownloadedAsset {
    /// Original URL
    pub url: String,
    /// Local file path where asset was saved
    pub local_path: PathBuf,
    /// MIME type detected from HTTP headers
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size: u64,
    /// SHA256 hash of content (first 12 hex chars used in filename)
    pub content_hash: String,
}

/// Download configuration
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Output directory for downloaded files
    pub output_dir: PathBuf,
    /// Subdirectory for images
    pub images_dir: String,
    /// Subdirectory for documents
    pub documents_dir: String,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: u64,
    /// Timeout for each download in seconds
    pub timeout_secs: u64,
    /// Maximum concurrent downloads (default: 3 for HDD)
    pub concurrency_limit: usize,
    /// User-Agent string for HTTP requests
    pub user_agent: String,
    /// URL glob patterns to include (empty = allow all)
    pub include_patterns: Vec<String>,
    /// URL glob patterns to exclude (always applied)
    pub exclude_patterns: Vec<String>,
    /// TLS/HTTP2 fingerprint profile
    pub h2_profile: Profile,
    /// Strategy for naming downloaded asset files
    pub asset_naming: AssetNamingStrategy,
    /// Maximum number of retry attempts for transient network errors
    pub max_retries: u32,
    /// Base delay for exponential backoff in milliseconds
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff in milliseconds
    pub backoff_max_ms: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./downloads"),
            images_dir: "images".to_string(),
            documents_dir: "documents".to_string(),
            max_file_size: 50 * 1024 * 1024,
            timeout_secs: 30,
            concurrency_limit: 3,
            user_agent: format!("WebCrawlerStaticPages/{}", env!("CARGO_PKG_VERSION")),
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            h2_profile: Profile::Chrome145,
            asset_naming: AssetNamingStrategy::default(),
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10_000,
        }
    }
}

// ============================================================================
// Extension: ScraperConfig → DownloadConfig (moved from the retired
// infrastructure config shim — issue #1099)
// ============================================================================

impl crate::domain::config::ScraperConfig {
    /// Build a `DownloadConfig` from this scraper configuration.
    ///
    /// This is the single source of truth for mapping ScraperConfig → DownloadConfig,
    /// eliminating duplication between the orchestrator and fallback paths.
    /// Lives in `adapters::downloader` next to `DownloadConfig` so `domain::config`
    /// does not depend on `adapters` (inward-only).
    pub fn to_download_config(&self) -> DownloadConfig {
        DownloadConfig {
            output_dir: self.output_dir.clone(),
            timeout_secs: self.download_timeout_secs,
            max_file_size: self.max_file_size.unwrap_or(50 * 1024 * 1024),
            concurrency_limit: self.download_concurrency,
            include_patterns: self.asset_include_patterns.clone(),
            exclude_patterns: self.asset_exclude_patterns.clone(),
            h2_profile: self.asset_h2_profile,
            asset_naming: self.asset_naming,
            ..Default::default()
        }
    }
}

/// Asset downloader
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
    /// Crawl-scoped successful-download registry (#782). One `Downloader`
    /// lives for the whole crawl (shared via `Arc` across all concurrent
    /// pages), so this map guarantees each canonical asset URL is fetched at
    /// most once: the first occurrence runs the download and records the
    /// asset here; concurrent arrivals wait on that result through the
    /// [`OnceCell`](tokio::sync::OnceCell); later occurrences (e.g. another
    /// page that references the same asset) reuse the already-written file.
    ///
    /// Only **successful** outcomes are cached. Failures are returned
    /// (typed, unmodified) and leave the cell uninitialized, so `run_download`'s
    /// retry policy is not bypassed and later pages are free to retry.
    downloaded_urls: DashMap<String, std::sync::Arc<OnceCell<DownloadedAsset>>>,
    /// Insertion order + O(1) membership for FIFO eviction. Only populated
    /// when the cache is bounded; the unbounded legacy path skips it entirely
    /// (no per-URL strings, no mutex traffic).
    asset_cache_order: std::sync::Mutex<InsertionLedger>,
    /// URLs with a download currently executing (RAII-guarded). Distinguishes
    /// an in-flight cell — which must never be evicted, or concurrent callers
    /// would open duplicate connections — from an ABANDONED uninitialized cell
    /// left behind by a permanent failure (`run_download` deliberately leaves
    /// failures uncached so later pages may retry).
    in_flight_downloads: DashMap<String, ()>,
    /// Maximum cached entries. `usize::MAX` = unbounded (legacy behavior,
    /// byte-identical to pre-cap releases).
    asset_cache_capacity: usize,
}

impl std::fmt::Debug for Downloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Downloader")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Downloader {
    /// Create a new downloader with configuration.
    ///
    /// Pre-creates output directories once to avoid runtime contention.
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Io` if directory creation fails.
    /// Returns `ScraperError::Config` if HTTP client build fails.
    pub fn new(config: DownloadConfig) -> Result<Self> {
        // Directories are created lazily on the first actual download (issue
        // #606): when no assets are downloaded we must not litter the CWD with
        // empty `output/` + subdir trees.
        //
        // Delegates to the bounded constructor with `usize::MAX` so both paths
        // share ONE wreq client builder — the SSRF policy (#703) can never
        // diverge between them.
        Self::with_asset_cache_capacity(config, usize::MAX)
    }

    /// Bounded-cache constructor: the dedup cache evicts oldest entries past
    /// `capacity` (Q3 materiality decision — downloaded_urls was the only
    /// structure whose measured growth crossed 50 MB at realistic long-run
    /// scale). Residual: an evicted URL may be re-downloaded if encountered
    /// again later (documented trade-off, design D2 PARTIAL).
    ///
    /// # Errors
    /// Propagates HTTP-client construction failures.
    pub fn with_asset_cache_capacity(config: DownloadConfig, capacity: usize) -> Result<Self> {
        let builder = Client::builder()
            .emulation(config.h2_profile)
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent);
        // SSRF guard (#703): identical policy to `new` — same client
        // construction, only the cache bound differs. Applied through the
        // domain `SsrfGuard` port.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to build http client: {e}")))?;
        Ok(Self::from_parts(client, config, capacity))
    }

    fn from_parts(
        client: wreq::Client,
        config: DownloadConfig,
        asset_cache_capacity: usize,
    ) -> Self {
        Self {
            client,
            config,
            downloaded_urls: DashMap::new(),
            asset_cache_order: std::sync::Mutex::new(InsertionLedger::default()),
            in_flight_downloads: DashMap::new(),
            asset_cache_capacity,
        }
    }

    /// Effective dedup-cache bound (#1120). `usize::MAX` marks the legacy
    /// unbounded mode; composition roots assert a finite value so long-lived
    /// processes can never reintroduce the linear-growth path.
    #[must_use]
    pub const fn asset_cache_capacity(&self) -> usize {
        self.asset_cache_capacity
    }

    /// Insert into the dedup cache through the bounded-cache discipline:
    /// records insertion order and enforces the capacity bound. Probe/test-only
    /// writer; production writes flow through `download`, which performs the
    /// same tracking inline.
    #[cfg(test)]
    pub(crate) fn insert_cached_cell(
        &self,
        url: String,
        cell: std::sync::Arc<OnceCell<DownloadedAsset>>,
    ) {
        self.downloaded_urls.insert(url.clone(), cell);
        if self.asset_cache_capacity != usize::MAX {
            // A poisoned ledger still grants exclusive access, and recovering
            // it keeps FIFO eviction working; skipping the push would leave the
            // cache unbounded for the rest of the run.
            let mut order = self
                .asset_cache_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            order.push_new(url);
        }
        self.evict_over_capacity();
    }

    /// FIFO-evict oldest entries while over capacity.
    /// Uninitialized cells are skipped (a download is still in flight —
    /// evicting them would allow a duplicate connection); they rotate to the
    /// back of the order queue instead.
    pub(crate) fn evict_over_capacity(&self) {
        if self.asset_cache_capacity == usize::MAX
            || self.downloaded_urls.len() <= self.asset_cache_capacity
        {
            return;
        }
        // Recover rather than skip: the guard still grants exclusive access, and
        // a poisoned ledger that stops evicting turns a bounded cache unbounded.
        let mut order = self
            .asset_cache_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Eviction classes:
        // - initialized cell -> evictable (normal FIFO victim),
        // - uninitialized + actively downloading (`in_flight_downloads`) ->
        //   never evicted (a duplicate connection would open); rotated back,
        // - uninitialized + NOT in flight -> abandoned failure zombie
        //   (`run_download` leaves failures uncached on purpose); evicted so
        //   error-heavy workloads stay bounded and later pages can retry.
        // The scan visits at most one full rotation of the queue, so an
        // all-in-flight excess cannot spin forever.
        let excess = self.downloaded_urls.len() - self.asset_cache_capacity;
        let mut evicted = 0;
        let mut deferred: VecDeque<String> = VecDeque::new();
        let max_scan = order.len();
        let mut scanned = 0;
        while evicted < excess && scanned < max_scan {
            scanned += 1;
            let Some(candidate) = order.pop_oldest() else {
                break;
            };
            let state = self
                .downloaded_urls
                .get(&candidate)
                .map(|e| {
                    if e.value().get().is_some() {
                        CellState::Initialized
                    } else if self.in_flight_downloads.contains_key(&candidate) {
                        CellState::InFlight
                    } else {
                        CellState::Abandoned
                    }
                })
                .unwrap_or(CellState::Gone);
            match state {
                CellState::InFlight => {
                    // Actively downloading: never evict; rotate to the back.
                    deferred.push_back(candidate);
                },
                CellState::Initialized | CellState::Abandoned | CellState::Gone => {
                    self.downloaded_urls.remove(&candidate);
                    order.forget(&candidate);
                    evicted += 1;
                },
            }
        }
        for url in deferred {
            order.requeue(url);
        }
    }

    /// Lazily create a download subdir only when a real asset is about to be
    /// persisted (issue #606). Avoids littering the CWD with empty dirs when
    /// no assets are downloaded.
    ///
    /// # Errors
    /// Returns `ScraperError::Io` if directory creation fails.
    fn ensure_subdir(&self, path: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(path).map_err(ScraperError::Io)
    }

    /// Disambiguate a filename that already exists on disk by appending a short
    /// content-hash suffix (preserving the extension).
    fn resolve_collision(
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
    fn generate_filename(
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

impl crate::domain::ports::AssetDownloaderPort for Downloader {
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
    > {
        let urls = urls.to_vec();
        Box::pin(async move {
            let results = self.download_batch(&urls).await;
            let assets = results
                .into_iter()
                .filter_map(|r| r.ok())
                .map(|a| {
                    let asset_type = crate::adapters::detector::detect_from_url(&a.url);
                    let asset_type_str = match asset_type {
                        crate::adapters::detector::AssetType::Image => "image",
                        crate::adapters::detector::AssetType::Document => "document",
                        crate::adapters::detector::AssetType::Unknown => "unknown",
                    };
                    crate::domain::entities::DownloadedAsset {
                        url: a.url,
                        local_path: a.local_path.to_string_lossy().into_owned(),
                        asset_type: asset_type_str.to_string(),
                        size: a.size,
                    }
                })
                .collect();
            Ok(assets)
        })
    }
}

/// The seam that lets `application` stop naming [`Downloader`] directly.
///
/// `domain::asset_downloader_factory` owns the trait and the
/// [`DefaultAssetDownloaderFactory`](crate::domain::asset_downloader_factory::DefaultAssetDownloaderFactory)
/// type; this is its only implementation.
/// `ScraperConfig::to_download_config` (in adapters::downloader, next to
/// `DownloadConfig`) stays the single mapping source, so
/// the factory path and the historical inline path build byte-identical
/// clients.
impl crate::domain::asset_downloader_factory::AssetDownloaderFactory
    for crate::domain::asset_downloader_factory::DefaultAssetDownloaderFactory
{
    fn build(
        &self,
        config: &crate::domain::config::ScraperConfig,
    ) -> crate::error::Result<Arc<dyn crate::domain::ports::AssetDownloaderPort>> {
        Ok(Arc::new(Downloader::new(config.to_download_config())?))
    }
}

/// Convert a Response into a stream of bytes
fn into_stream(response: Response) -> impl StreamExt<Item = wreq::Result<bytes::Bytes>> {
    response.bytes_stream()
}

/// Check if a URL matches include/exclude filters.
///
/// If `include_patterns` is empty, all URLs pass the include check.
/// `exclude_patterns` are always applied (deny wins).
fn url_matches_filters(url: &ValidUrl, includes: &[String], excludes: &[String]) -> bool {
    if excludes.iter().any(|p| pattern_matches_asset(url, p)) {
        return false;
    }
    if includes.is_empty() {
        return true;
    }
    includes.iter().any(|p| pattern_matches_asset(url, p))
}

/// Match a URL against a pattern, supporting both extension globs (`*.pdf`)
/// and the standard host/path glob from `domain::pattern_matching`.
fn pattern_matches_asset(url: &ValidUrl, pattern: &str) -> bool {
    let p = pattern.trim();
    // Extension glob: *.ext (but NOT host globs like *.example.com which contain a dot after the prefix)
    if let Some(ext) = p.strip_prefix("*.") {
        if !ext.is_empty() && !ext.contains('.') {
            // The URL is already parsed — the old `Url::parse(url)` re-parse
            // per pattern per asset is gone (#1117).
            let last = url.path().rsplit('/').next().unwrap_or("");
            let low = last.to_ascii_lowercase();
            let ext_low = ext.to_ascii_lowercase();
            return low.ends_with(&format!(".{ext_low}"));
        }
    }
    crate::domain::matches_pattern(url.as_str(), pattern)
}

/// Compute exponential backoff delay with jitter.
fn compute_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    use rand::Rng;
    use std::cmp::min;

    // Exponential: base * 2^(attempt-1), clamped to max. Shift is capped at 62
    // to avoid panic from shifting u64 by >= 64 bits (attempt=1 is shift=0).
    let shift = (attempt - 1).min(62);
    let delay_ms = min(base_ms.saturating_mul(1u64 << shift), max_ms);
    // Add jitter: 75%-125% of delay, then clamp final result to max_ms
    let jitter = delay_ms / 4;
    let offset = rand::rng().random_range(0..=jitter.saturating_mul(2));
    let final_ms = min(
        delay_ms.saturating_sub(jitter).saturating_add(offset),
        max_ms,
    );
    Duration::from_millis(final_ms)
}

/// Derive a slug from the last path segment of a URL.
fn derive_slug_from_url(url: &ValidUrl) -> String {
    // The URL is already parsed — the old `Url::parse(url).ok()` re-parse
    // and its silent empty-slug fallback are gone (#1117).
    url.path()
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && *s != "/")
        .map(String::from)
        .unwrap_or_default()
}

/// UTF-8 safe percent-decoding for Content-Disposition filenames.
/// Uses the `percent-encoding` crate for correct handling of truncated
/// sequences, multi-byte chars, and all edge cases in RFC 5987/3986.
fn percent_decode_utf8(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .into_owned()
}

/// Sanitize an untrusted filename into a safe single path component.
///
/// Thin delegate over
/// [`crate::domain::crawler_port::filename::sanitize_filename_component`]
/// — the single source of truth for filename sanitization. Returns an
/// empty string when nothing safe remains (fully hostile input such as
/// `.` / `..`), so callers like [`Downloader::generate_filename`] fall back to
/// hash-based naming.
fn sanitize_filename(name: &str) -> String {
    crate::domain::crawler_port::filename::sanitize_filename_component(name).unwrap_or_default()
}

/// Parse `filename=` from a Content-Disposition header value.
///
/// RFC 6266 / 5987: parameter names are case-insensitive. We lowercase
/// the header value before matching to handle `Filename=`, `FILENAME=`, etc.
fn parse_content_disposition_header(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();

    // Try filename*=utf-8''encoded first (RFC 5987)
    if let Some(start) = lower.find("filename*=utf-8''") {
        let encoded = &value[start + "filename*=utf-8''".len()..];
        let name: String = encoded
            .chars()
            .take_while(|c| *c != ';' && *c != ' ')
            .collect();
        let decoded = percent_decode_utf8(&name);
        return Some(decoded);
    }

    // Try filename="name" or filename=name (case-insensitive)
    let after = lower.find("filename=")?;
    let rest = &value[after + "filename=".len()..];
    let name = if let Some(inner) = rest.strip_prefix('"') {
        // Quoted: filename="name.pdf"
        let end = inner.find('"')?;
        &inner[..end]
    } else {
        // Unquoted: filename=name.pdf
        rest.split(';').next().unwrap_or(rest).trim()
    };

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// MIME type to file extension mapping
fn mime_type_to_extension(mime: &str) -> Option<String> {
    let mime = mime.trim();
    match mime {
        "image/jpeg" | "image/jpg" => Some("jpg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "image/svg+xml" => Some("svg".to_string()),
        "image/bmp" => Some("bmp".to_string()),
        "image/tiff" => Some("tiff".to_string()),
        "image/x-icon" => Some("ico".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        "application/msword" => Some("doc".to_string()),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("docx".to_string())
        },
        "application/vnd.ms-excel" => Some("xls".to_string()),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx".to_string())
        },
        "application/vnd.ms-powerpoint" => Some("ppt".to_string()),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("pptx".to_string())
        },
        "text/csv" => Some("csv".to_string()),
        "application/vnd.oasis.opendocument.text" => Some("odt".to_string()),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("ods".to_string()),
        "application/epub+zip" => Some("epub".to_string()),
        "application/rtf" => Some("rtf".to_string()),
        "text/plain" => Some("txt".to_string()),
        "application/json" => Some("json".to_string()),
        "application/xml" | "text/xml" => Some("xml".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests_bounded_cache;
#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_download;
#[cfg(test)]
mod tests_memory_probe;
#[cfg(test)]
mod tests_naming;
