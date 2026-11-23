//! On-disk representation of the v2 record store: the versioned envelope, the
//! injectable filesystem backend, the exclusive lock sentinel, and the
//! backup-first v1→v2 migration. Kept separate from the public persistence API
//! in [`super`] so each file stays a cohesive unit.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::domain::exporter::{DomainRecords, RawRecord, RecordStoreError};
use crate::domain::page_state::{PageStatus, MIGRATED_V1_RUN_ID};
use crate::domain::record_transition;

use super::{now_millis, RecordStore};

/// Versioned on-disk envelope (D2, frozen).
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StoreFile {
    pub(super) version: u32,
    pub(super) domain_key: String,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
    pub(super) records: DomainRecords,
}

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

impl RecordStore {
    /// Explicit v1→v2 migration (Gate 2/SC5): backup FIRST, map every
    /// `processed_urls[i]` to a Committed record under
    /// [`MIGRATED_V1_RUN_ID`], then save v2 atomically. The original v1
    /// bytes stay untouched until the rename succeeds; the backup covers
    /// even that window.
    ///
    /// Called only from [`super::RecordStore::load_under_lock`], which already
    /// holds the store lock; it deliberately does not re-acquire it (`flock(2)`
    /// on a second descriptor would self-deadlock).
    pub(super) fn migrate_v1(
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
}
