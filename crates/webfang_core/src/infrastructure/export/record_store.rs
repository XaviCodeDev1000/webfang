//! JSON-per-domain v2 record store (D2, frozen contracts).
//!
//! Persists one [`crate::domain::exporter::RawRecord`] per canonical URL under
//! a versioned envelope:
//!
//! ```json
//! { "version": 2, "domain_key": "example.com", "created_at": 0,
//!   "updated_at": 0, "records": { "<canonical_url>": { ... } } }
//! ```
//!
//! # Durability scope
//!
//! Writes are atomic via same-directory temp file + rename(2). The temp file
//! is fsynced before the rename and the parent directory is fsynced after it,
//! so a successful `save` is durable across power loss, not only process death.
//!

use std::fs;
use std::path::PathBuf;

use tracing::debug;

use crate::domain::exporter::DomainRecords;
use crate::domain::record_transition;

#[path = "record_store_file.rs"]
mod record_store_file;
use record_store_file::{LoadedStore, RealFs, StoreFile, StoreFs, StoreLock};

// Backwards-compat shim (ADR-0012-B 3.H): the record DTOs and their error
// moved to `domain::exporter`; re-export them so `infrastructure::export::*`
// paths keep resolving until the one-minor-version shim window closes.
pub use crate::domain::exporter::RecordStorePort;

/// Envelope format version this module reads and writes.
pub(crate) const CURRENT_VERSION: u32 = 2;

pub use crate::domain::page_state::MIGRATED_V1_RUN_ID;

/// Typed failures of the record store. `load_or_init` propagates every one
/// of them as an operator-visible error; it never fabricates an empty store
/// over unreadable state (Gate 2).
///
/// The enum itself lives in `domain::exporter` next to the port it guards;
/// this re-export keeps the historical `record_store::RecordStoreError` path
/// resolving during the shim window (ADR-0012-B 3.H).
pub use crate::domain::exporter::RecordStoreError;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Monotonic per-process suffix for crash-sensitive sibling filenames.
///
/// A clock reading can repeat and a PID is not unique inside one process, so
/// both the temp file and the v1 backup name use this counter instead.
pub(crate) fn unique_suffix() -> String {
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}", std::process::id(), nonce)
}

/// JSON-per-domain v2 persistence (D2).
///
/// Construction is deliberately lazy/infallible (#393 contract): strictness
/// lives in [`RecordStore::load`], never in the constructor.
#[derive(Clone)]
pub struct RecordStore {
    domain_key: String,
    state_dir: Option<PathBuf>,
    fs: std::sync::Arc<dyn StoreFs>,
}

impl std::fmt::Debug for RecordStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordStore")
            .field("domain_key", &self.domain_key)
            .field("state_dir", &self.state_dir)
            .finish_non_exhaustive()
    }
}

impl RecordStore {
    /// Create a store for a normalized domain key, rooted at the platform
    /// cache dir (`$CACHE/webfang/state`).
    #[must_use]
    pub fn new(domain_key: impl Into<String>) -> Self {
        Self {
            domain_key: domain_key.into(),
            state_dir: None,
            fs: std::sync::Arc::new(RealFs),
        }
    }

    /// Override the base directory (tests / explicit state dirs).
    #[must_use]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = Some(dir);
        self
    }

    /// Test-only constructor with an injected I/O backend.
    #[cfg(test)]
    pub(crate) fn with_fs(domain_key: &str, dir: PathBuf, fs: std::sync::Arc<dyn StoreFs>) -> Self {
        Self {
            domain_key: domain_key.to_string(),
            state_dir: Some(dir),
            fs,
        }
    }

    /// Full path of this domain's state file.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        let mut path = match &self.state_dir {
            Some(dir) => dir.clone(),
            None => {
                let mut p = crate::domain::paths::cache_base_dir()
                    .unwrap_or_else(|| PathBuf::from(".cache"));
                p.push("webfang");
                p.push("state");
                p
            },
        };
        path.push(format!("{}.json", self.domain_key));
        path
    }

    fn tmp_path(&self, final_path: &std::path::Path) -> PathBuf {
        // Shared with the v1 backup name: both siblings must stay unique even
        // when two writers enter the critical section in the same clock tick.
        let mut name = final_path.file_name().map_or_else(
            || "state.json".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        name.push_str(&format!(".tmp-{}", unique_suffix()));
        final_path.with_file_name(name)
    }

    /// Delete stale `.tmp-*` siblings left by killed processes. Best-effort;
    /// temp files are NEVER read as state.
    fn gc_tmp_files(&self, state_path: &std::path::Path) {
        let prefix = format!(
            "{}.tmp-",
            state_path.file_name().unwrap_or_default().to_string_lossy()
        );
        if let Some(parent) = state_path.parent() {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with(&prefix) {
                        debug!(file = %name.to_string_lossy(), "GC stale record-store tmp file");
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    /// Persist records atomically: serialize → same-dir tmp (`<pid>-<nonce>`)
    /// → write_all + flush → rename(2). No fsync — documented non-goal.
    /// An exclusive lock is held across the whole write.
    ///
    /// # Errors
    ///
    /// Returns [`RecordStoreError`] if directory creation, serialization,
    /// writing, or the final rename fails. The previous state file (if any)
    /// is never partially modified.
    pub fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RecordStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let _lock = StoreLock::acquire(&path)?;
        crate::cli::crash_points::hit(crate::cli::crash_points::WHILE_HOLDING_LOCK);
        // `save` carries no loaded records, so this is the one write path that
        // still reads the envelope back for `created_at`.
        self.save_locked(records, Self::existing_created_at(&path), now_millis())
    }

    /// Save while a lock is already held (shared by [`Self::update`], which
    /// passes the `created_at` its own load already read).
    fn save_locked(
        &self,
        records: &DomainRecords,
        created_at: Option<i64>,
        updated_at: i64,
    ) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        let envelope = StoreFile {
            version: CURRENT_VERSION,
            domain_key: self.domain_key.clone(),
            created_at: created_at.unwrap_or_else(now_millis),
            updated_at,
            records: records.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&envelope)
            .map_err(|_| RecordStoreError::Corrupt { path: path.clone() })?;
        let tmp = self.tmp_path(&path);
        // Crash-injection: leave a TRUNCATED tmp before dying (surviving
        // occurrences overwrite with the full payload below).
        if crate::cli::crash_points::is_armed_for(crate::cli::crash_points::MID_STATE_FILE_WRITE) {
            let _ = self.fs.write(&tmp, &bytes[..bytes.len() / 2]);
            crate::cli::crash_points::hit(crate::cli::crash_points::MID_STATE_FILE_WRITE);
        }
        self.fs
            .write(&tmp, &bytes)
            .map_err(|source| RecordStoreError::Io {
                path: tmp.clone(),
                source,
            })?;
        crate::cli::crash_points::hit(crate::cli::crash_points::TMP_WRITTEN_PRE_RENAME);
        if let Err(source) = self.fs.rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(RecordStoreError::Io { path, source });
        }
        debug!(domain = %self.domain_key, records = records.len(), "saved record store");
        Ok(())
    }

    /// The `created_at` field of an already-parsed envelope.
    ///
    /// Extracted from the JSON `Value` rather than from a typed envelope so the
    /// same rule serves both versions: a v1 file has no typed `created_at` and
    /// a v2 file always does. [`Self::existing_created_at`] adds the read for
    /// callers that only have a path.
    fn created_at_of(envelope: &serde_json::Value) -> Option<i64> {
        envelope.get("created_at")?.as_i64()
    }

    /// Read `created_at` back from the file at `path`.
    ///
    /// Only [`Self::save`] needs this: a plain save arrives without a load, so
    /// the envelope metadata it must preserve is not in memory yet. [`Self::update`]
    /// gets the same value from its own load and never calls this.
    fn existing_created_at(path: &std::path::Path) -> Option<i64> {
        let bytes = fs::read(path).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Self::created_at_of(&value)
    }

    /// Load records for this domain.
    ///
    /// * Missing file → fresh empty store (nothing to discard).
    /// * Stale `.tmp-*` siblings are garbage-collected, never read.
    /// * Corrupt JSON → [`RecordStoreError::Corrupt`].
    /// * Version 1 → explicit backup-first migration to v2 (D2/SC5).
    /// * Any other version → [`RecordStoreError::UnsupportedVersion`]; no
    ///   silent discard exists (Gate 2). Callers wanting the policy apply
    ///   [`RecordStore::load_or_init`], which warns with the named path.
    ///
    /// # Errors
    ///
    /// See above; per-record invariant violations never error — they are
    /// quarantined with a `warn!` naming url + invariant + path.
    pub fn load(&self) -> Result<DomainRecords, RecordStoreError> {
        let path = self.state_path();
        if !path.exists() {
            debug!(domain = %self.domain_key, "no record store yet; starting fresh");
            return Ok(DomainRecords::new());
        }
        // #1230: the read belongs to the same critical section as the write.
        // A lock-free `load` paired with a locked `save` is precisely the
        // read-modify-write window that lost records between processes.
        let _lock = StoreLock::acquire(&path)?;
        self.load_under_lock(&path).map(|loaded| loaded.records)
    }

    /// Load with the store lock already held.
    ///
    /// Callers: [`Self::load`] and [`Self::update`]. Never acquires the lock
    /// itself — `StoreLock` is `flock(2)` on a fresh file descriptor, so
    /// re-entering it from the same process would self-deadlock.
    ///
    /// This is the ONE read of the store's critical section: the envelope's
    /// `created_at` leaves here inside the returned [`LoadedStore`], so a
    /// following write never re-reads the file (T8).
    fn load_under_lock(&self, path: &std::path::Path) -> Result<LoadedStore, RecordStoreError> {
        self.gc_tmp_files(path);
        let bytes = fs::read(path).map_err(|source| RecordStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let envelope: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(envelope) => envelope,
            Err(_) => {
                crate::application::resume::preserve_abandoned_bytes(
                    path,
                    &bytes,
                    "record_store_corrupt_envelope",
                );
                return Err(RecordStoreError::Corrupt {
                    path: path.to_path_buf(),
                });
            },
        };
        match envelope
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32
        {
            CURRENT_VERSION => {
                let file: StoreFile = match serde_json::from_value(envelope) {
                    Ok(file) => file,
                    Err(_) => {
                        crate::application::resume::preserve_abandoned_bytes(
                            path,
                            &bytes,
                            "record_store_corrupt_records",
                        );
                        return Err(RecordStoreError::Corrupt {
                            path: path.to_path_buf(),
                        });
                    },
                };
                Ok(LoadedStore {
                    records: Self::validate_and_quarantine(file.records, path),
                    created_at: Some(file.created_at),
                })
            },
            1 => {
                // A v1 envelope has no typed `created_at`; the field the writer
                // used to read back is taken from the bytes already in hand.
                let created_at = Self::created_at_of(&envelope);
                self.migrate_v1(path, &bytes, created_at)
            },
            found => {
                // M3 (#1617): the DOWNGRADE rewrite. `load_or_init` turns this
                // error into a fresh store, and the next `update()` renames a
                // new file over this one — so the newer schema's bytes are
                // destroyed by the older binary that refused them. The v1->v2
                // upgrade above is backup-first; this direction had no backup at
                // all, which is exactly the asymmetry the audit named.
                crate::application::resume::preserve_abandoned_bytes(
                    path,
                    &bytes,
                    "record_store_unsupported_version",
                );
                Err(RecordStoreError::UnsupportedVersion {
                    path: path.to_path_buf(),
                    found,
                })
            },
        }
    }

    /// Atomic read-modify-write under the store's exclusive lock (#1230).
    ///
    /// `StoreLock` is held across load → mutate → save, so two `--resume`
    /// processes sharing one `--state-dir` can never both start from a stale
    /// snapshot and clobber each other. The closure observes exactly what is
    /// durable at the instant the lock is taken and its result is persisted
    /// before the lock is released.
    ///
    /// Unlike [`Self::load_or_init`] this never degrades to an empty view:
    /// beginning a *transaction* from a fabricated empty state is how
    /// unreadable state silently becomes a whole-file overwrite (P8-3).
    ///
    /// # Errors
    ///
    /// [`RecordStoreError`] from the load, from `mutate`, or from the save.
    /// When `mutate` returns `Err`, nothing is written and the file on disk
    /// is untouched.
    pub fn update(
        &self,
        mutate: &mut dyn FnMut(&mut DomainRecords) -> Result<(), RecordStoreError>,
    ) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RecordStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let _lock = StoreLock::acquire(&path)?;
        // Same crash point `save` uses, and it must stay here: `update` is now
        // the live write path, so a crash point that only `save` hits is a
        // guard off the critical path — the RC-3 defect class this sprint
        // exists to delete. Without this line the SIGKILL-while-holding-the-
        // lock cell of the crash matrix silently stops testing anything.
        crate::cli::crash_points::hit(crate::cli::crash_points::WHILE_HOLDING_LOCK);
        // Re-checked INSIDE the lock: another writer may have created the
        // file between the caller's decision and our acquisition.
        let loaded = if path.exists() {
            self.load_under_lock(&path)?
        } else {
            LoadedStore {
                records: DomainRecords::new(),
                created_at: None,
            }
        };
        // The load above is the ONLY read of the critical section: it carries
        // the envelope's `created_at` forward, so the write below does not read
        // the file it would have to re-parse just for that field (T8).
        let mut records = loaded.records;
        mutate(&mut records)?;
        self.save_locked(&records, loaded.created_at, now_millis())
    }

    /// Load the persisted records, refusing to substitute an empty store.
    ///
    /// A corrupt or unsupported-version file is an operator-visible error:
    /// the typed cause is returned unchanged and logged at `error` level.
    /// The original bytes stay on disk for forensics.
    ///
    /// # Errors
    ///
    /// Every [`RecordStoreError`] from [`Self::load`] is propagated.
    pub fn load_or_init(&self) -> Result<DomainRecords, RecordStoreError> {
        self.load().inspect_err(|err| {
            tracing::error!(error = %err, "record store unreadable; refusing to start with fresh state — original file preserved");
        })
    }

    /// D2/E6 invariant table applied at the single persistence seam.
    /// Violations are QUARANTINED (dropped from the in-memory view and
    /// re-drivable next run), never panics: `warn!` names url + invariant +
    /// file path. The rule itself lives in the pure ADR-0014 state machine
    /// ([`crate::domain::record_transition`]); this wrapper
    /// contributes only the file-path context and the `warn!` I/O.
    fn validate_and_quarantine(records: DomainRecords, path: &std::path::Path) -> DomainRecords {
        let (kept, quarantined) = record_transition::partition_valid(records);
        for entry in &quarantined {
            tracing::warn!(
                url = %entry.url,
                invariant = entry.invariant,
                file = %path.display(),
                "quarantining record-store entry with impossible state"
            );
        }
        kept
    }

    /// Count of `COMMITTED` records — the compat replacement for v1's stored
    /// `total_exported` counter (A3): derived, never authoritative. The
    /// rule lives in the pure ADR-0014 state machine; this associated
    /// function is a delegating alias kept for its existing callers.
    #[must_use]
    pub fn derived_total_exported(records: &DomainRecords) -> u64 {
        record_transition::derived_total_exported(records)
    }
}

impl RecordStorePort for RecordStore {
    fn update(
        &self,
        mutate: &mut dyn FnMut(&mut DomainRecords) -> Result<(), RecordStoreError>,
    ) -> Result<(), RecordStoreError> {
        RecordStore::update(self, mutate)
    }

    fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError> {
        RecordStore::save(self, records)
    }

    fn load(&self) -> Result<DomainRecords, RecordStoreError> {
        RecordStore::load(self)
    }

    fn load_or_init(&self) -> Result<DomainRecords, RecordStoreError> {
        RecordStore::load_or_init(self)
    }
}

#[cfg(test)]
#[path = "record_store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "record_store_recovery_tests.rs"]
mod recovery_tests;
