//! Persistence mode — domain control-plane unifying `--resume`/`--state-dir`
//! and `--checkpoint-interval`/`--no-checkpoint` without changing persisted formats.
//!
//! Pure resolver: `PersistenceMode::from_config(&ResumeConfig, &Path)` maps four
//! CLI flags to an exhaustive enum. No IO, no async, no logging, deterministic.
//! When the caller needs to know whether `--state-dir` was silently ignored,
//! use `from_config_with_notes` and act on the returned [`ResolverNotes`]
//! (the CLI layer re-emits the `warn!` from there; batch/MCP callers drop it).
//!
//! [`ResolverNotes`]: crate::domain::persistence::ResolverNotes
//!
//! The `ResumeConfig` value object is owned by domain (it is the input the
//! resolver consumes); the application layer (`CrawlLimits::resume_config`)
//! is responsible for translating CLI flags into a `ResumeConfig`. Domain
//! never imports from `crate::application::*` — the Clean Architecture rule
//! `infrastructure → adapters → application → domain` is inward only.
//!
//! ADR-0014 also relocated the persistence ports and record DTOs here from
//! `domain::exporter`, making this module the single domain-side home for
//! persistence: the `PersistenceMode`/`ResumeConfig` resolver, the
//! `StateStorePort`/`RecordStorePort` seams, and the
//! `RawRecord`/`LastError`/`DomainRecords` contract types.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::error::ErrorClass;
use crate::domain::page_state::{PageStatus, PersistedRecord, MIGRATED_V1_RUN_ID};

/// Input for [`PersistenceMode::from_config`].
///
/// Exactly the four CLI flags slice 5c unified. Held by domain so the
/// resolver stays independent of the application layer's `CrawlLimits`
/// (which carries ~12 unrelated composition fields: concurrency,
/// rate-limit, headers, cookies, patterns, etc.).
///
/// # Construction
///
/// Application code is the only place that names this type directly.
/// `CrawlLimits::resume_config()` is the canonical entry point. Tests
/// build it inline because the matrix is small.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeConfig {
    /// `--resume` (env `WEBFANG_RESUME`).
    pub resume: bool,
    /// `--state-dir <PATH>` (env `WEBFANG_STATE_DIR`).
    pub state_dir: Option<PathBuf>,
    /// `--checkpoint-interval <PAGES>` — 0 disables checkpointing.
    pub checkpoint_interval: u64,
    /// `--no-checkpoint` — explicit opt-out (overrides `checkpoint_interval`).
    pub no_checkpoint: bool,
}

/// Checkpoint configuration — directory and interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointCfg {
    /// Directory holding the per-seed checkpoint file
    /// (`crawl_checkpoint_<seed-hash>.json`, F-01 scoped so concurrent
    /// `--resume` runs for different sites sharing one dir cannot collide).
    pub dir: PathBuf,
    /// Pages between automatic checkpoint saves (0 is disabled, but this struct
    /// is only constructed when checkpoint is enabled, so interval > 0 here).
    pub interval: u64,
}

/// Unified persistence control-plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceMode {
    /// No persistence active.
    Disabled,
    /// Resume only (`RecordStore` via `apply_resume_mode`).
    Resume {
        /// Effective resume state directory.
        dir: PathBuf,
    },
    /// Checkpoint only (`Engine::with_checkpoint`).
    Checkpoint {
        /// Checkpoint configuration.
        cfg: CheckpointCfg,
    },
    /// Both resume and checkpoint active.
    Full {
        /// Effective resume state directory.
        resume_dir: PathBuf,
        /// Checkpoint configuration.
        checkpoint: CheckpointCfg,
    },
}

/// Side-channel notes returned by [`PersistenceMode::from_config_with_notes`].
///
/// The resolver stays pure (no IO, no logging): every decision the caller
/// might need to surface is returned as data. Today that is exactly one
/// case — `--state-dir` that had no effect, i.e. the resolved mode is
/// `Disabled` while a `state_dir` was passed (no `--resume` opt-in and no
/// checkpoint opt-in, or checkpointing disabled via interval 0 /
/// `--no-checkpoint`). The CLI layer re-emits the `warn!` from
/// `ignored_state_dir`; callers without user flags (batch, MCP, tests) drop
/// the notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolverNotes {
    /// The `--state-dir` value that was ignored (`Some` only when
    /// `state_dir.is_some() && !resume`), to be logged by the caller.
    pub ignored_state_dir: Option<PathBuf>,
}

impl PersistenceMode {
    /// Pure resolver — no IO, no logging.
    ///
    /// `default_state_dir` is the caller-supplied fallback (XDG_CACHE_HOME or
    /// `~/.cache/webfang/state`). Truth table over
    /// `(resume, state_dir, interval, no_checkpoint)` (F-01, Option A):
    ///
    /// - `no_checkpoint` or `interval == 0` → `Disabled` (checkpoint off).
    /// - `(false, None, N>0, false)` → `Disabled`: the default interval alone
    ///   never enables checkpointing, so repeat crawls stay deterministic
    ///   unless the operator opts in. Rationale: defaulting the interval to 0
    ///   instead would silently disable `--resume`-without-explicit-interval
    ///   (`checkpoint_enabled = interval != 0 && !no_checkpoint`), breaking
    ///   the resume UX — the opt-in gate achieves determinism without that.
    /// - `(false, Some(dir), N>0, false)` → `Checkpoint{dir}`: an explicit
    ///   `--state-dir` is the checkpoint opt-in and is honoured.
    /// - `(true, _, N>0, false)` → `Resume`/`Full` over `dir or default`.
    ///
    /// Callers that owe the operator a `warn!` for an ignored `--state-dir`
    /// must use [`from_config_with_notes`](Self::from_config_with_notes)
    /// instead (the note is set exactly when the mode is `Disabled`).
    pub fn from_config(cfg: &ResumeConfig, default_state_dir: &Path) -> Self {
        Self::from_config_with_notes(cfg, default_state_dir).0
    }

    /// Pure resolver with side-channel notes — no IO, no logging.
    ///
    /// Returns the resolved mode plus [`ResolverNotes`]: `ignored_state_dir`
    /// is `Some` exactly when `--state-dir` had no effect (resolved mode is
    /// `Disabled` while a `state_dir` was passed). The caller that knows about
    /// user flags emits `warn!(state_dir = ?notes.ignored_state_dir,
    /// "ignoring --state-dir without --resume")` from it; all other callers
    /// ignore the notes.
    pub fn from_config_with_notes(
        cfg: &ResumeConfig,
        default_state_dir: &Path,
    ) -> (Self, ResolverNotes) {
        let checkpoint_enabled = cfg.checkpoint_interval != 0 && !cfg.no_checkpoint;
        let mode = match (cfg.resume, checkpoint_enabled) {
            (false, false) => Self::Disabled,
            (true, false) => {
                let dir = cfg
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                Self::Resume { dir }
            },
            // F-01 opt-in gate: without --resume, checkpointing requires
            // an explicit --state-dir; the default interval alone stays
            // Disabled so repeat crawls are deterministic.
            (false, true) => match cfg.state_dir.clone() {
                Some(dir) => {
                    let checkpoint = CheckpointCfg {
                        dir,
                        interval: cfg.checkpoint_interval,
                    };
                    Self::Checkpoint { cfg: checkpoint }
                },
                None => Self::Disabled,
            },
            (true, true) => {
                let resume_dir = cfg
                    .state_dir
                    .clone()
                    .unwrap_or_else(|| default_state_dir.to_path_buf());
                let checkpoint = CheckpointCfg {
                    dir: resume_dir.clone(),
                    interval: cfg.checkpoint_interval,
                };
                Self::Full {
                    resume_dir,
                    checkpoint,
                }
            },
        };
        // `--state-dir` is ignored exactly when it had no effect: the
        // resolved mode is Disabled while a state_dir was passed.
        let ignored_state_dir = match (&mode, &cfg.state_dir) {
            (Self::Disabled, Some(dir)) => Some(dir.clone()),
            _ => None,
        };
        (mode, ResolverNotes { ignored_state_dir })
    }

    /// `true` for `Resume` and `Full`.
    #[must_use]
    pub fn is_resume(&self) -> bool {
        matches!(self, Self::Resume { .. } | Self::Full { .. })
    }

    /// Checkpoint config when checkpoint is enabled.
    #[must_use]
    pub fn checkpoint_cfg(&self) -> Option<&CheckpointCfg> {
        match self {
            Self::Checkpoint { cfg }
            | Self::Full {
                checkpoint: cfg, ..
            } => Some(cfg),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence ports and record DTOs (relocated from `domain::exporter` by
// ADR-0014, Decision 1). `domain/exporter.rs` keeps `pub use` re-exports so
// the merged public paths (ADR-0012-B 3.H, ADR-0013 consumers) keep
// compiling for at least one release cycle.
//
// The persisted per-URL record DTOs live in the domain because both the
// application layer (`export_factory`, `resume`) and the infra store mutate
// them through the typestate lifecycle. The JSON-per-domain store itself
// stays in `infrastructure::export::record_store` and implements the port
// below (WafInspectorPort pattern, #993): concrete in infra, seam in domain.
// ---------------------------------------------------------------------------

/// Keyed by canonical URL; deterministic serialization order.
pub type DomainRecords = BTreeMap<String, RawRecord>;

/// A classified error attached to a persisted record (SC6 taxonomy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// Operational classification of the failure.
    pub class: ErrorClass,
    /// Human-readable failure message.
    pub message: String,
}

/// The persisted per-URL record — the nine frozen spec fields plus the
/// additive degradation marker (T6, issue #3117). `degraded_reason` is an
/// `Option` because the marker is ADDITIVE (plan S4): a store written before
/// it existed loads unchanged (`#[serde(default)]`) and a record that did not
/// degrade writes no field at all (`skip_serializing_if`), so the pre-T6
/// shape survives byte-for-byte. Unknown fields are still rejected on load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRecord {
    /// Original URL as discovered.
    pub url: String,
    /// Resolved canonical URL; www/apex unify here.
    pub canonical_url: String,
    /// uuid v4 of the run that last touched this record; `"migrated-v1"`
    /// for migrated legacy entries.
    pub run_id: String,
    /// Hash of the serialized payload; dedup/reconciliation key (D3).
    pub content_hash: Option<String>,
    /// Persisted attempt count. NOT actuated — retry policy is a non-goal.
    pub attempts: u32,
    /// Lifecycle position (the ONLY machine state that serializes).
    pub status: PageStatus,
    /// Last classified failure, cleared on success.
    pub last_error: Option<LastError>,
    /// Output path recorded at the EXPORTED checkpoint.
    pub output_location: Option<String>,
    /// Unix millis UTC of the last persist.
    pub updated_at: i64,
    /// Why this page's text is the RAW fallback rather than cleaned text
    /// (T6, issue #3117 / finding E9), as the `SemanticError` variant name
    /// (`ChunkTooLarge`, `Tokenize`, `InvalidSpan`).
    ///
    /// `None` means "not degraded" — the record makes no claim either way
    /// about a page this run never cleaned, which is the honest reading for a
    /// plain (non `--clean-ai`) run. Cleared when a later run cleans the same
    /// URL successfully, so the field always describes the LAST write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

impl RawRecord {
    /// Validated constructor for a fresh DISCOVERED lifecycle (#876).
    ///
    /// This is the choke point every writer of new records MUST pass through:
    /// an empty or whitespace-only URL is structurally meaningless identity
    /// and is rejected with [`RecordStoreError::InvalidRecord`] instead of
    /// ever becoming retrievable state. Fields default to the honest fresh
    /// shape (`attempts = 0`, no hash, no output location, no error); callers
    /// mutate from there via the typestate lifecycle.
    ///
    /// # Errors
    ///
    /// [`RecordStoreError::InvalidRecord`] when `url` or `canonical_url` is
    /// empty after trimming.
    pub fn new_discovered(
        url: &str,
        canonical_url: &str,
        run_id: &str,
        updated_at: i64,
    ) -> Result<Self, RecordStoreError> {
        if url.trim().is_empty() {
            return Err(RecordStoreError::InvalidRecord {
                reason: "url must not be empty",
            });
        }
        if canonical_url.trim().is_empty() {
            return Err(RecordStoreError::InvalidRecord {
                reason: "canonical_url must not be empty",
            });
        }
        Ok(Self {
            url: url.to_string(),
            canonical_url: canonical_url.to_string(),
            run_id: run_id.to_string(),
            content_hash: None,
            attempts: 0,
            status: PageStatus::Discovered,
            last_error: None,
            output_location: None,
            updated_at,
            degraded_reason: None,
        })
    }
}

impl PersistedRecord for RawRecord {
    fn status(&self) -> PageStatus {
        self.status
    }

    fn output_location(&self) -> Option<&str> {
        self.output_location.as_deref()
    }

    fn content_hash(&self) -> Option<&str> {
        self.content_hash.as_deref()
    }

    fn has_last_error(&self) -> bool {
        self.last_error.is_some()
    }

    fn attempts(&self) -> u32 {
        self.attempts
    }

    fn set_status(&mut self, status: PageStatus) {
        self.status = status;
    }

    fn is_migrated_v1(&self) -> bool {
        self.run_id == MIGRATED_V1_RUN_ID
    }
}

/// Typed failures of the record store. Callers apply the named-path
/// fresh-start policy (`load_or_init`) on [`RecordStoreError::Corrupt`] and
/// [`RecordStoreError::UnsupportedVersion`] — never silently (Gate 2).
#[derive(Debug, thiserror::Error)]
pub enum RecordStoreError {
    /// Filesystem failure at a known path.
    #[error("record store I/O error at {path}: {source}")]
    Io {
        /// Path the failing operation targeted.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The state file exists but is not valid JSON / not a valid store.
    #[error("record store file is corrupt or not valid JSON: {path}")]
    Corrupt {
        /// Path of the unreadable file.
        path: PathBuf,
    },
    /// The state file carries a version neither 1 nor 2.
    #[error("record store file has unsupported version {found}: {path}")]
    UnsupportedVersion {
        /// Path of the offending file.
        path: PathBuf,
        /// Version found in the envelope.
        found: u32,
    },
    /// Creating the pre-migration backup failed; migration aborted.
    #[error("failed to create v1 backup at {path}: {source}")]
    Backup {
        /// Intended backup path.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// A writer attempted to create a record whose identity is structurally
    /// meaningless (#876): an empty URL can never address a fetched page, so
    /// it is rejected at the domain boundary instead of becoming persisted
    /// state (fail-closed, same class as unparseable legacy input).
    #[error("invalid record rejected at domain boundary: {reason}")]
    InvalidRecord {
        /// The invariant that failed (English, log-oriented).
        reason: &'static str,
    },
}

/// Domain-owned seam over the JSON-per-domain record store (ADR-0012-B 3.H).
///
/// `application` consumes this trait; the concrete
/// `infrastructure::export::record_store::RecordStore` implements it. Object
/// safe: call sites hold `&dyn RecordStorePort`.
pub trait RecordStorePort: Send + Sync {
    /// Atomic read-modify-write: load, mutate, save — all under the store's
    /// exclusive lock (#1230 / F-07).
    ///
    /// This is the ONLY port method that may be used to change persisted
    /// state. The `load` + `save` pair below is not a transaction: two
    /// `--resume` processes that each load before either saves will have the
    /// last writer silently discard the other's records.
    ///
    /// `mutate` observes exactly the records durable at the instant the lock
    /// is taken. Returning `Err` writes nothing and leaves the file
    /// untouched.
    ///
    /// Unlike [`Self::load_or_init`] this never degrades to an empty view:
    /// starting a transaction from a fabricated empty state is how unreadable
    /// state silently becomes a whole-file overwrite (P8-3).
    ///
    /// # Errors
    /// [`RecordStoreError`] from the load, from `mutate`, or from the save.
    fn update(
        &self,
        mutate: &mut dyn FnMut(&mut DomainRecords) -> Result<(), RecordStoreError>,
    ) -> Result<(), RecordStoreError>;

    /// Persist `records` atomically (temp file + rename(2), no fsync).
    ///
    /// Whole-file snapshot write. Prefer [`Self::update`] for any change
    /// derived from previously persisted state.
    ///
    /// # Errors
    /// [`RecordStoreError`] on filesystem failure or serialization error.
    fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError>;

    /// Load the persisted records for this domain.
    ///
    /// # Errors
    /// [`RecordStoreError::Corrupt`] / [`RecordStoreError::UnsupportedVersion`]
    /// for unreadable state; callers apply the named-path fresh-start policy.
    fn load(&self) -> Result<DomainRecords, RecordStoreError>;

    /// Load prior history, surfacing unreadable state as an operator-visible
    /// error instead of an empty in-memory view. The original bytes stay on
    /// disk for forensics.
    ///
    /// # Errors
    /// [`RecordStoreError::Corrupt`] / [`RecordStoreError::UnsupportedVersion`]
    /// and every I/O or backup failure from the underlying store.
    fn load_or_init(&self) -> Result<DomainRecords, RecordStoreError>;
}

/// Domain-owned seam over the legacy JSON-per-domain state store (#1097).
///
/// `cli` constructs the concrete
/// `infrastructure::export::state_store::StateStore` through
/// `application::container::build_state_store`; `cli` consumes it
/// through this port only.
///
/// The live production surface of this port is
/// [`get_state_path`](Self::get_state_path) alone: `record_store_bridge`
/// (`cli::scrape_flow`) reads the state path to derive the directory and the
/// domain of the v2 record store. `mark_processed`/`is_processed`/`load` were
/// pruned with zero production callers (#2243 and its follow-up #2491).
///
/// Object safe: call sites hold `&dyn StateStorePort` or
/// `Arc<dyn StateStorePort>`.
///
/// Resume filtering does not flow through this port: it flows through the v2
/// seam [`RecordStorePort::load_or_init`]. Errors surface as
/// [`crate::error::ScraperError`].
pub trait StateStorePort: Send + Sync {
    /// Full path to the domain state JSON file.
    fn get_state_path(&self) -> PathBuf;
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
