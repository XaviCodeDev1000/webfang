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

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::ValidUrl;
use crate::error::{Result, ScraperError};
use dashmap::DashMap;
use tokio::sync::OnceCell;
use wreq::Client;
use wreq_util::Profile;

/// Entries retained per Asset-tier permit in the dedup cache (Q3 MEASURE
/// FIRST: BEFORE data measured ~600 B/entry; default tier 3 x 8_192 =
/// 24_576 entries ~= 15 MiB ceiling, far below the 50 MB materiality line).
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
            output_dir: self.scoped_output_dir(),
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

    /// Default download directory, relocated under the tenant-or-run root.
    ///
    /// S17: only the untouched default (`./output`) is relocated. A value the
    /// operator set — `--output`, a config key, a library caller's
    /// `with_output_dir` — is preserved exactly, which is what keeps CLI and
    /// environment overrides ahead of the scope.
    fn scoped_output_dir(&self) -> PathBuf {
        if self.output_dir == std::path::Path::new("./output") {
            if let Some(scoped) = crate::domain::paths::TenantScope::current().downloads_dir() {
                return scoped;
            }
        }
        self.output_dir.clone()
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
    /// Maximum cached entries. [`new`](Self::new) always sets the finite
    /// [`cache::DEFAULT_ASSET_CACHE_CAPACITY`]; `usize::MAX` = the unbounded
    /// legacy mode, reachable only through
    /// [`with_asset_cache_capacity`](Self::with_asset_cache_capacity).
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
    /// The dedup cache is bounded to
    /// [`cache::DEFAULT_ASSET_CACHE_CAPACITY`] entries (S17): an unbounded
    /// default made every caller that skipped a composition root grow the
    /// registry linearly with the crawl. Call
    /// [`with_asset_cache_capacity`](Self::with_asset_cache_capacity) for an
    /// explicit bound.
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
        // Both constructors share ONE wreq client builder — the SSRF policy
        // (#703) can never diverge between them.
        Self::with_asset_cache_capacity(config, cache::DEFAULT_ASSET_CACHE_CAPACITY)
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
mod cache;
mod fetch;
mod naming;
pub use cache::{asset_cache_capacity, ASSET_CACHE_ENTRIES_PER_PERMIT};
pub(crate) use cache::{CellState, InsertionLedger};
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
