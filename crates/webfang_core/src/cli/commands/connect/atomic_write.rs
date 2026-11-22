use std::path::Path;

/// Monotonic counter backing the connect temp-name suffix.
///
/// `pid` alone is not unique inside a multi-threaded process, and two
/// writers a nanosecond apart can derive the same name from a clock. A
/// per-process monotonic counter makes every temp name unique for the
/// lifetime of the process; the pid keeps concurrent PROCESSES disjoint.
static TMP_NONCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Attempts to mint a temp name before failing loud on a persistent
/// pre-plant (see [`create_temp`]).
pub(super) const TEMP_CREATE_ATTEMPTS: usize = 3;

/// Mint the temp path for one write: `.{file_name}.connect-{pid}-{nonce}.tmp`
/// next to the target.
///
/// The dot prefix keeps the temp out of a plain listing, and the pid +
/// monotonic nonce make the name unguessable to a would-be planter while
/// still obvious to the GC sweep (the strict `.{file_name}.connect-`
/// prefix). The nonce is what makes the name UNIQUE: pid alone collides
/// between two writes in one multi-threaded process, and the collision is
/// silent — one write renames the temp away and the other's rename fails
/// with ENOENT.
pub(super) fn temp_path(dir: &Path, file_name: &str) -> std::path::PathBuf {
    let nonce = TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!(
        ".{file_name}.connect-{}-{nonce}.tmp",
        std::process::id()
    ))
}

/// Create the temp file, retrying with a fresh name when the candidate was
/// pre-planted. Returns the temp path actually used.
///
/// `next_temp` mints the candidate name; production passes [`temp_path`],
/// tests pass a deterministic sequence so the retry arm is exercised
/// without predicting the process counter.
///
/// O_EXCL rejection (`create_new`, see [`write_temp`]) is the pre-plant
/// signal: someone with write access to the config directory left a file —
/// typically a symlink — at the exact name this write was about to use,
/// hoping the writer would truncate through it (CWE-377). That never
/// happens here: the candidate is abandoned, `next_temp` mints the next
/// name, and the write lands there. The retry is bounded by
/// [`TEMP_CREATE_ATTEMPTS`]; a collision that survives every attempt is
/// reported loud rather than downgraded to a truncating open.
///
/// A pre-planted candidate is NOT removed on the way out: the path belongs
/// to whoever created it, and unlinking it would only race that writer.
/// Only a temp this call actually created is cleaned up.
pub(super) fn create_temp(
    mut next_temp: impl FnMut() -> std::path::PathBuf,
    target: &Path,
    contents: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    let mut temp = next_temp();
    let mut attempts_left = TEMP_CREATE_ATTEMPTS;
    loop {
        match write_temp(&temp, target, contents) {
            Ok(()) => return Ok(temp),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts_left > 1 => {
                attempts_left -= 1;
                temp = next_temp();
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    let _ = std::fs::remove_file(&temp);
                }
                return Err(e);
            },
        }
    }
}

/// Best-effort sweep of stale connect temps beside the freshly written
/// config.
///
/// A connect killed between create and rename leaves its temp behind — a
/// full copy of the operator's config. The next successful write reclaims
/// every sibling sharing the strict `.{file_name}.connect-` prefix,
/// whatever pid or nonce left it (the pre-fix `.{file}.connect-{pid}.tmp`
/// names included). Sweeping NEVER fails the operation it follows: the
/// config is already renamed into place, a temp is never read as config,
/// and the subcommands run without a subscriber, so a GC problem must not
/// turn a successful connect into an error. A live sibling's temp may lose
/// the race; that writer's rename then fails loud — the loss is never
/// silent.
pub(super) fn gc_connect_temps(parent: &Path, prefix: &str, current: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(prefix) || entry.path().as_path() == current {
            continue;
        }
        tracing::debug!(file = %name, "GC stale connect temp file");
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Write `contents` to `path` atomically: O_EXCL temp file in the same
/// directory, then rename over the target. A same-filesystem rename is
/// atomic, so a PROCESS crash (panic, kill, failed write) leaves either the
/// old file or the new one — never a truncated config. A POWER crash is
/// NOT covered: neither the temp file nor the parent directory is fsynced,
/// so after power loss the rename may be durable while the new contents
/// are not.
///
/// The rename replaces the destination INODE and does not follow symlinks, so
/// two things are corrected before the rename: a symlinked config is resolved
/// to its target (otherwise a stow/chezmoi link is silently replaced by a
/// regular file and later hand-edits stop landing in the dotfiles tree), and
/// the temp file inherits the target's permissions (otherwise an operator who
/// restricted `config.toml` to `0600` silently gets `0644`).
///
/// The temp name is unique per process AND per write (pid + monotonic
/// nonce, see [`temp_path`]) and is created with O_EXCL: a pre-planted
/// name — a symlink left by someone with write access to the config
/// directory — is refused, never truncated into, and the write retries
/// [`TEMP_CREATE_ATTEMPTS`] times on a fresh name before failing loud (see
/// [`create_temp`]). After a successful rename the stale `.connect-` temps
/// left by killed runs are swept best-effort (see [`gc_connect_temps`]).
pub(super) fn write_atomically(path: &Path, contents: &str) -> Result<(), std::io::Error> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");

    // The temp is cleaned up on BOTH error arms: leaving it behind turns a
    // failed connect into a stray file full of the operator's config. A
    // pre-planted candidate is the exception — it is not ours to remove
    // (see `create_temp`).
    let temp = create_temp(|| temp_path(parent, file_name), &target, contents)?;
    match std::fs::rename(&temp, &target) {
        Ok(()) => {
            gc_connect_temps(parent, &format!(".{file_name}.connect-"), &temp);
            Ok(())
        },
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        },
    }
}

/// Create `temp` carrying the permissions the finished config must have, then
/// write `contents` into it.
///
/// The temp is created with `create_new` (O_EXCL): if anything already
/// exists at `temp` — a pre-planted file or symlink — the open fails with
/// [`std::io::ErrorKind::AlreadyExists`] and NOTHING is written, so a
/// symlink plant cannot redirect this write at an arbitrary victim file
/// (CWE-377). `truncate` is deliberately absent: O_EXCL guarantees a
/// brand-new file, so there is never anything to truncate, and keeping the
/// flag would suggest the code truncates a file it refuses to touch.
/// `write` is required alongside `create_new` — O_EXCL without an access
/// mode is EINVAL.
///
/// A NEW config is created `0600` from the start: even though `AuthSource` has
/// no inline-secret variant today, the file carries keyring account names and
/// LAN allowlists, and the repo already treats `0600` as the convention for
/// credential-bearing files. An EXISTING config keeps whatever mode the
/// operator chose, copied onto the temp before the rename.
pub(super) fn write_temp(temp: &Path, target: &Path, contents: &str) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(temp)?;
    std::io::Write::write_all(&mut file, contents.as_bytes())?;
    // Closed before the chmod: on Windows an open handle blocks it.
    drop(file);

    if let Ok(metadata) = std::fs::metadata(target) {
        std::fs::set_permissions(temp, metadata.permissions())?;
    }
    Ok(())
}
