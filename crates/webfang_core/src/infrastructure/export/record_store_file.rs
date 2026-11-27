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

use super::{now_millis, unique_suffix, RecordStore};

/// Versioned on-disk envelope (D2, frozen).
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StoreFile {
    pub(super) version: u32,
    pub(super) domain_key: String,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
    pub(super) records: DomainRecords,
}

/// What a locked load hands back: the records plus the envelope metadata a
/// following write must carry forward.
///
/// `created_at` travels WITH the records it was parsed from, so the writer
/// never has to re-read the file the load just read (T8: one read per
/// [`super::RecordStore::update`]). `None` means the envelope on disk carried
/// no `created_at` (a fresh store, or a v1 file), which is exactly the case
/// where the old read-back fell through to `now_millis`.
#[derive(Debug)]
pub(super) struct LoadedStore {
    pub(super) records: DomainRecords,
    pub(super) created_at: Option<i64>,
}

/// Injectable filesystem surface so unit tests can fault-inject typed-error
/// plumbing without real disk state (D6: integration fidelity comes from the
/// SIGKILL harness, mocks only cover error plumbing).
pub(crate) trait StoreFs: Send + Sync {
    /// Create/truncate + write_all + flush + `sync_all` on the temp file.
    /// The write is durable BEFORE the caller's rename(2).
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()>;
    /// rename(2) — the atomicity and commit-point primitive — followed by a
    /// `sync_all` of the parent directory so the new name survives a crash.
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
}

/// Flush the directory entry a rename created. POSIX exposes a directory as
/// a file for `fsync(2)`; platforms without that facility keep rename
/// atomicity but cannot explicitly sync the directory entry.
fn sync_parent_dir(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Production backend over [`std::fs`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RealFs;

impl StoreFs for RealFs {
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(from, to)?;
        sync_parent_dir(to)
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
    /// Acquire an exclusive OS file lock at the owner-only `<state>.lock`.
    ///
    /// On Unix the permanent sentinel is created and kept at mode `0o600`: it
    /// holds the owning PID, which must not be readable by other users. This
    /// does not change the advisory nature of the lock described below.
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
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut lock_file = options
            .open(&lock_path)
            .map_err(|source| RecordStoreError::Io {
                path: lock_path.clone(),
                source,
            })?;
        #[cfg(unix)]
        {
            // The sentinel is permanent: mode() only applies at creation, so
            // tighten sentinels left behind by older versions on every acquire.
            use std::os::unix::fs::PermissionsExt as _;
            lock_file
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|source| RecordStoreError::Io {
                    path: lock_path.clone(),
                    source,
                })?;
        }
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
    ///
    /// `created_at` is the legacy envelope's own field, read from the bytes
    /// `load_under_lock` already has: the migration carries it into the v2
    /// envelope instead of reading the file a second time for it.
    pub(super) fn migrate_v1(
        &self,
        path: &std::path::Path,
        original_bytes: &[u8],
        created_at: Option<i64>,
    ) -> Result<LoadedStore, RecordStoreError> {
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

        let updated_at = created_at.unwrap_or_else(now_millis);
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
            "{}.v1.bak.{stamp}.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            unique_suffix()
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
        self.save_locked(&records, created_at, updated_at)?;
        tracing::info!(
            domain = %self.domain_key,
            migrated = records.len(),
            backup = %backup_path.display(),
            "v1 state file migrated to v2 record store"
        );
        Ok(LoadedStore {
            records: Self::validate_and_quarantine(records, path),
            created_at,
        })
    }
}

#[cfg(all(test, unix))]
mod lock_permission_tests {
    use super::*;

    #[test]
    fn preexisting_world_readable_sentinel_is_tightened_on_acquire() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::TempDir::new().expect("tempdir");
        let state = dir.path().join("state.json");
        let mut os_lock = state.as_os_str().to_owned();
        os_lock.push(".lock");
        let lock_path = std::path::PathBuf::from(os_lock);
        std::fs::write(&lock_path, b"").expect("seed sentinel");
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644))
            .expect("set seed mode");

        let lock = StoreLock::acquire(&state).expect("acquire lock");
        let mode = std::fs::metadata(&lock_path)
            .expect("lock metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "lock sentinel must not be readable by group or others"
        );
        drop(lock);
    }
}

#[cfg(test)]
mod backup_name_tests {
    use super::*;
    use std::sync::Arc;

    #[cfg_attr(miri, ignore)] // copy_file_range (syscall 326) unsupported by Miri (miri#4183)
    #[test]
    fn repeated_migrations_keep_distinct_backup_names() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("state.json");
        let legacy = br#"{"processed_urls":["https://example.com/a"]}"#;
        std::fs::write(&path, legacy).expect("write legacy state");
        let store = RecordStore::with_fs("test", dir.path().to_path_buf(), Arc::new(RealFs));

        for _ in 0..20 {
            store.migrate_v1(&path, legacy, None).expect("migration");
        }

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read state dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".v1.bak."))
            .collect();
        assert_eq!(
            backups.len(),
            20,
            "each migration must keep its own backup, even within one millisecond"
        );
    }
}
