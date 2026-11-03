//! Atomic rename for the file saver.
//!
//! The temp file is written in the SAME directory as the final path and
//! renamed into place; Windows needs a bounded retry because a concurrent
//! reader (search indexer, antivirus, another scrape) can hold the
//! destination open. See the Windows advisory CI lane — this arm cannot
//! execute on the Linux checkout.

use std::path::Path;

/// Move the temp file onto its final name.
///
/// Windows-only concern (XP-F-03, #1608): a concurrent reader holding the
/// destination open — search indexer, antivirus, another scrape consuming
/// the same file — makes `rename` fail with a sharing/lock violation even
/// though it would succeed moments later. Retry a few times with a short
/// backoff before giving up. Unix renames are atomic and never fail this
/// way, so the unix build keeps a single plain rename.
///
/// COMPILE-PROOF ONLY: this branch cannot execute on the Linux checkout —
/// the Windows advisory CI lane must verify the retry behavior at runtime.
#[cfg(windows)]
pub(super) fn rename_temp_into_place(from: &Path, to: &Path) -> std::io::Result<()> {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const MAX_ATTEMPTS: u32 = 5;
    const BASE_BACKOFF_MS: u64 = 10;

    for attempt in 1..=MAX_ATTEMPTS {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
                ) && attempt < MAX_ATTEMPTS =>
            {
                std::thread::sleep(std::time::Duration::from_millis(
                    BASE_BACKOFF_MS * u64::from(attempt),
                ));
            },
            Err(e) => return Err(e),
        }
    }
    unreachable!("the loop always returns on its last attempt")
}

#[cfg(not(windows))]
pub(super) fn rename_temp_into_place(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}
