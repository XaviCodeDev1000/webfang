//! JSON-per-domain v2 record store (D2, frozen contracts).
//!
//! Persists one [`RawRecord`] per canonical URL under a versioned envelope:
//!
//! ```json
//! { "version": 2, "domain_key": "example.com", "created_at": 0,
//!   "updated_at": 0, "records": { "<canonical_url>": { ... } } }
//! ```
//!
//! # Durability scope (loud non-goal)
//!
//! Writes are atomic against process death via same-directory temp file +
//! [`std::fs::rename`] (rename(2)). There is deliberately NO fsync: a power
//! loss may lose the last rename even though the process observed success.
//! Process-crash durability is the contract; power-loss durability is not.
//!

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::exporter::{DomainRecords, RawRecord};
use crate::domain::page_state::PageStatus;
use crate::domain::record_transition;

// Backwards-compat shim (ADR-0012-B 3.H): the record DTOs and their error
// moved to `domain::exporter`; re-export them so `infrastructure::export::*`
// paths keep resolving until the one-minor-version shim window closes.
pub use crate::domain::exporter::RecordStorePort;

/// Envelope format version this module reads and writes.
pub(crate) const CURRENT_VERSION: u32 = 2;

pub use crate::domain::page_state::MIGRATED_V1_RUN_ID;

/// Versioned on-disk envelope (D2, frozen).
#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    domain_key: String,
    created_at: i64,
    updated_at: i64,
    records: DomainRecords,
}

/// Typed failures of the record store. Callers apply the named-path
/// fresh-start policy (`load_or_init`) on [`RecordStoreError::Corrupt`] and
/// [`RecordStoreError::UnsupportedVersion`] — never silently (Gate 2).
///
/// The enum itself lives in `domain::exporter` next to the port it guards;
/// this re-export keeps the historical `record_store::RecordStoreError` path
/// resolving during the shim window (ADR-0012-B 3.H).
pub use crate::domain::exporter::RecordStoreError;

/// Injectable filesystem surface so unit tests can fault-inject typed-error
/// plumbing without real disk state (D6: integration fidelity comes from the
/// SIGKILL harness, mocks only cover error plumbing).
pub(crate) trait StoreFs: Send + Sync {
    /// Create/truncate + write_all + flush (page cache; NO fsync by contract).
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()>;
    /// rename(2) — the atomicity and commit-point primitive.
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
}

/// Production backend over [`std::fs`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RealFs;

impl StoreFs for RealFs {
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.flush()
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
}

/// RAII exclusive lock over the store's state file.
///
/// The lock file is a PERMANENT sentinel: it is created on demand and never
/// deleted. Removing it on drop — the previous behaviour — breaks mutual
/// exclusion, and #1230 is the symptom. `flock(2)` guards an **inode**, not a
/// path: once the holder unlinks the file, a second writer can `create()` a
/// NEW inode, lock it immediately, and enter the critical section while the
/// first writer (still blocked on the unlinked inode) is inside it. Two
/// threads in one process were observed destroying each other's state file
/// exactly that way, surfacing as `Io { NotFound }` from the loser's rename.
///
/// SIGKILL closes the fd and releases the OS lock (E3). The empty sentinel
/// stays behind, which is harmless and bounded: one per state file.
#[must_use]
pub(crate) struct StoreLock {
    handle: fs::File,
}

impl StoreLock {
    /// Acquire an exclusive OS file lock at `<state>.lock`.
    ///
    /// Semantics differ by platform (XP-F-04, #1608) — the lock is
    /// "advisory" only on POSIX:
    ///
    /// - **Unix**: `flock(2)` is advisory — it excludes only other
    ///   cooperating `flock` users, not arbitrary readers/writers.
    /// - **Windows**: fs2 maps `lock_exclusive` to `LockFileEx` byte-range
    ///   locks, which the OS enforces against other processes' file access.
    ///
    /// The mutual-exclusion contract of this store is between cooperating
    /// webfang processes on either platform, which both primitives provide.
    /// It is NOT a claim of protection against non-cooperating programs on
    /// unix.
    pub(crate) fn acquire(state_path: &std::path::Path) -> Result<Self, RecordStoreError> {
        let mut lock_path = state_path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        let mut lock_file =
            fs::File::create(&lock_path).map_err(|source| RecordStoreError::Io {
                path: lock_path.clone(),
                source,
            })?;
        let _ = write!(lock_file, "pid={}", std::process::id());
        lock_file
            .lock_exclusive()
            .map_err(|e| RecordStoreError::Io {
                path: lock_path.clone(),
                source: std::io::Error::other(format!("failed to acquire record-store lock: {e}")),
            })?;
        Ok(Self { handle: lock_file })
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Best-effort: failure must not mask the real result. The sentinel is
        // deliberately NOT removed — see the type-level doc above.
        let _ = FileExt::unlock(&self.handle);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
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
        // `pid` alone is not unique inside a multi-threaded process, and
        // `subsec_nanos` can repeat: two writers entering the critical section
        // a nanosecond apart produced the SAME tmp name, one renamed it away, and
        // the other's rename failed with ENOENT. A monotonic per-process counter
        // makes the name unique for the lifetime of the process.
        static TMP_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut name = final_path.file_name().map_or_else(
            || "state.json".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        name.push_str(&format!(".tmp-{}-{}", std::process::id(), nonce));
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
        self.save_locked(records, now_millis())
    }

    /// Save while a lock is already held (shared by [`Self::update`]).
    fn save_locked(
        &self,
        records: &DomainRecords,
        updated_at: i64,
    ) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        let envelope = StoreFile {
            version: CURRENT_VERSION,
            domain_key: self.domain_key.clone(),
            created_at: Self::existing_created_at(&path).unwrap_or_else(now_millis),
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

    fn existing_created_at(path: &std::path::Path) -> Option<i64> {
        let bytes = fs::read(path).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        value.get("created_at")?.as_i64()
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
        self.load_under_lock(&path)
    }

    /// Load with the store lock already held.
    ///
    /// Callers: [`Self::load`] and [`Self::update`]. Never acquires the lock
    /// itself — `StoreLock` is `flock(2)` on a fresh file descriptor, so
    /// re-entering it from the same process would self-deadlock.
    fn load_under_lock(&self, path: &std::path::Path) -> Result<DomainRecords, RecordStoreError> {
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
                Ok(Self::validate_and_quarantine(file.records, path))
            },
            1 => self.migrate_v1(path, &bytes),
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
        let mut records = if path.exists() {
            self.load_under_lock(&path)?
        } else {
            DomainRecords::new()
        };
        mutate(&mut records)?;
        self.save_locked(&records, now_millis())
    }

    /// E5 policy wrapper: corrupt or unsupported-version state files are
    /// converted to a fresh empty store — WITH a warning naming the file,
    /// never silently (Gate 2). The original bytes stay on disk for
    /// forensics; only the caller decides to overwrite them later.
    #[must_use]
    pub fn load_or_init(&self) -> DomainRecords {
        match self.load() {
            Ok(records) => records,
            Err(
                err @ (RecordStoreError::Corrupt { .. }
                | RecordStoreError::UnsupportedVersion { .. }),
            ) => {
                tracing::warn!(error = %err, "record store unreadable; starting FRESH per explicit policy — original file preserved for inspection");
                DomainRecords::new()
            },
            Err(RecordStoreError::Io { path, source }) => {
                tracing::warn!(error = %source, file = %path.display(), "record store I/O failure; starting FRESH per explicit policy");
                DomainRecords::new()
            },
            Err(RecordStoreError::Backup { path, source }) => {
                tracing::warn!(error = %source, file = %path.display(), "record store backup failure; starting FRESH per explicit policy");
                DomainRecords::new()
            },
            // Unreachable today: load() only surfaces Io/Corrupt/
            // UnsupportedVersion/Backup; InvalidRecord is produced by the
            // writer-side constructor. Matched exhaustively so a future
            // error variant can never silently fall through (fail-closed).
            Err(err @ RecordStoreError::InvalidRecord { .. }) => {
                tracing::warn!(error = %err, "record store rejected an invalid record; starting FRESH per explicit policy");
                DomainRecords::new()
            },
        }
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

    /// Explicit v1→v2 migration (Gate 2/SC5): backup FIRST, map every
    /// `processed_urls[i]` to a Committed record under
    /// [`MIGRATED_V1_RUN_ID`], then save v2 atomically. The original v1
    /// bytes stay untouched until the rename succeeds; the backup covers
    /// even that window.
    ///
    /// Called only from [`Self::load_under_lock`], which already holds the
    /// store lock; it deliberately does not re-acquire it (`flock(2)` on a
    /// second descriptor would self-deadlock).
    fn migrate_v1(
        &self,
        path: &std::path::Path,
        original_bytes: &[u8],
    ) -> Result<DomainRecords, RecordStoreError> {
        let legacy: serde_json::Value =
            serde_json::from_slice(original_bytes).map_err(|_| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?;
        let urls: Vec<String> = legacy
            .get("processed_urls")
            .cloned()
            .map(serde_json::from_value::<Vec<String>>)
            .transpose()
            .map_err(|_| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?
            .ok_or_else(|| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?;

        let updated_at = Self::existing_created_at(path).unwrap_or_else(now_millis);
        let mut migrated_mtime = None;
        // File mtime is the design-preferred updated_at when readable.
        if let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) {
            if let Ok(secs) = modified.duration_since(std::time::UNIX_EPOCH) {
                migrated_mtime = Some(i64::try_from(secs.as_millis()).unwrap_or_default());
            }
        }
        let updated_at = migrated_mtime.unwrap_or(updated_at);

        let mut records = DomainRecords::new();
        for url in urls {
            // #876: an empty legacy URL is malformed input of the same
            // class as other per-record invariant violations — quarantined
            // (dropped + warned), never promoted to Committed state. The
            // whole-file Corrupt policy stays reserved for unparseable
            // structure, so one bad entry never discards good neighbors.
            if record_transition::is_meaningless_identity(&url) {
                tracing::warn!(
                    file = %path.display(),
                    "v1 migration: dropping empty-string URL from legacy processed_urls (malformed legacy entry)"
                );
                continue;
            }
            records.insert(
                url.clone(),
                RawRecord {
                    url: url.clone(),
                    canonical_url: url,
                    run_id: MIGRATED_V1_RUN_ID.to_string(),
                    content_hash: None,
                    attempts: 1,
                    status: PageStatus::Committed,
                    last_error: None,
                    output_location: None,
                    updated_at,
                },
            );
        }

        // (1) Backup BEFORE anything else touches the live file.
        let stamp = now_millis();
        let backup_path = path.with_file_name(format!(
            "{}.v1.bak.{stamp}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::copy(path, &backup_path).map_err(|source| RecordStoreError::Backup {
            path: backup_path.clone(),
            source,
        })?;

        // (2)+(3) Save the upgraded envelope atomically. The lock is already
        // held by the load that routed us here (#1230).
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RecordStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        self.save_locked(&records, updated_at)?;
        tracing::info!(
            domain = %self.domain_key,
            migrated = records.len(),
            backup = %backup_path.display(),
            "v1 state file migrated to v2 record store"
        );
        Ok(Self::validate_and_quarantine(records, path))
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

    fn load_or_init(&self) -> DomainRecords {
        RecordStore::load_or_init(self)
    }
}

#[cfg(test)]
#[path = "record_store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "record_store_recovery_tests.rs"]
mod recovery_tests;
