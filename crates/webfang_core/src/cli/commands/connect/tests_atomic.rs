use super::atomic_write::*;
use super::fixtures::*;
use super::*;

/// A config an operator restricted to `0600` (the repo's convention for
/// credential-bearing files) must not silently become `0644` because
/// `rename(2)` replaces the destination inode.
#[cfg(unix)]
#[test]
fn write_provider_preserves_the_existing_config_file_mode() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, path, provider) = first_write_canonical();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("restrict the config like an operator would");

    assert_second_write_keeps_mode(
        &path,
        &provider,
        0o600,
        "the atomic rewrite downgraded the operator's 0600 config",
    );
}

/// The permission COPY is its own obligation, and a `0600` assertion cannot
/// see it: the temp is created `0600`, so asserting `0600` passes whether the
/// writer copied the operator's mode or just kept the default. An operator
/// running a wider mode — group-readable `0640` in a shared setup — would be
/// silently narrowed to `0600` with every test still green.
#[cfg(unix)]
#[test]
fn write_provider_preserves_a_config_mode_wider_than_the_temp_default() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, path, provider) = first_write_canonical();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
        .expect("widen the config the way a group-shared setup would");

    assert_second_write_keeps_mode(
        &path,
        &provider,
        0o640,
        "the atomic rewrite narrowed an operator's 0640 config to the 0600 temp default",
    );
}

/// A brand-new config is created `0600` from the start: even though
/// `AuthSource` has no inline-secret variant today, the file carries
/// keyring account names and LAN allowlists.
#[cfg(unix)]
#[test]
fn write_provider_creates_a_new_config_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, path, _provider) = first_write_canonical();

    let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "a newly created credential-bearing config must be created 0600"
    );
}

/// `rename(2)` does not follow symlinks, so a config managed by stow /
/// chezmoi was being replaced by a regular file, silently detaching the
/// link from the operator's dotfiles tree.
#[cfg(unix)]
#[test]
fn write_provider_writes_through_a_symlinked_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let managed = dir.path().join("dotfiles").join("webfang-config.toml");
    std::fs::create_dir_all(managed.parent().expect("parent")).expect("make dotfiles dir");
    std::fs::write(&managed, "format = \"markdown\"\n").expect("seed the managed file");
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&managed, &link).expect("symlink the config");

    let provider = test_provider("connect-a1b2");
    write_provider(&link, &provider).expect("write through the symlink");

    assert!(
        std::fs::symlink_metadata(&link)
            .expect("lstat")
            .file_type()
            .is_symlink(),
        "the atomic rewrite replaced the operator's symlink with a regular file"
    );
    let resolved = std::fs::read_to_string(&managed).expect("read the resolved target");
    assert!(
        resolved.contains("id = \"connect-a1b2\"") && resolved.contains("format ="),
        "the new content must land in the resolved target\n---\n{resolved}"
    );
}

/// A failed write must not leave its temp file behind: the temp holds the
/// full config text, so a stale `.config.toml.connect-<pid>.tmp` is a copy
/// of the operator's configuration sitting next to it. The name is now
/// unique per write (pid + nonce), so a later write cannot even overwrite
/// it — only the failure cleanup below reclaims it. The retry arm never
/// fires here: nothing pre-planted the name, so the first candidate wins
/// and the rename failure removes it.
#[test]
fn write_atomically_leaves_no_temp_file_when_the_rename_fails() {
    let (dir, target) = temp_config();
    // A non-empty directory at the destination makes rename(2) fail only
    // after the temp has already been written.
    std::fs::create_dir(&target).expect("make a directory where a file is expected");
    std::fs::write(target.join("occupied"), b"x").expect("make it non-empty");

    write_atomically(&target, "id = \"connect-a1b2\"\n")
        .expect_err("renaming a file over a non-empty directory must fail");

    let leftovers: Vec<String> = dir_file_names_matching(dir.path(), |name| {
        name.starts_with(".config.toml") && name.ends_with(".tmp")
    });
    assert!(
        leftovers.is_empty(),
        "a failed atomic write left its temp file behind: {leftovers:?}"
    );
}

/// THE CWE-377 regression: a symlink planted at the temp path is never
/// followed or truncated through. The open fails with `AlreadyExists`
/// (O_EXCL) and the victim file is left byte-for-byte intact.
#[cfg(unix)]
#[test]
fn write_temp_refuses_a_preplanted_symlink_without_touching_the_victim() {
    let (dir, target) = temp_config();
    let temp = dir.path().join(".config.toml.connect-1-0.tmp");
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, b"VICTIM\n").expect("seed the victim");
    std::os::unix::fs::symlink(&victim, &temp).expect("plant the symlink");

    let err = write_temp(&temp, &target, "id = \"connect-a1b2\"\n")
        .expect_err("a pre-planted temp must be refused, never truncated into");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read_to_string(&victim).expect("the victim must still be readable"),
        "VICTIM\n",
        "the write followed the planted symlink and destroyed the victim"
    );
    assert!(!target.exists(), "nothing may be written through the plant");
}

/// The name carries pid + monotonic nonce, so two writes in the SAME
/// process (same pid) can never collide on one temp — the old pid-only
/// name let one write rename the temp away under the other, whose
/// rename then failed ENOENT.
#[test]
fn temp_path_is_unique_per_write_within_one_process() {
    let dir = tempfile::tempdir().expect("temp dir");
    let first = temp_path(dir.path(), "config.toml");
    let second = temp_path(dir.path(), "config.toml");

    assert_ne!(first, second, "two writes must never share a temp name");
    let expected_prefix = format!(".config.toml.connect-{}-", std::process::id());
    for name in [&first, &second] {
        let name = name
            .file_name()
            .expect("a temp path has a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with(&expected_prefix),
            "the temp must name the process and a nonce: {name}"
        );
        assert!(name.ends_with(".tmp"), "the temp keeps its suffix: {name}");
    }
    assert_eq!(
        first.parent(),
        second.parent(),
        "the temp must stay beside its target for the rename to be atomic"
    );
}

/// A pre-planted candidate is abandoned for the NEXT name: the write
/// lands in a different temp (never through the plant, which keeps its
/// bytes — unlinking it would only race its owner).
#[test]
fn create_temp_retries_with_a_fresh_name_when_the_candidate_is_planted() {
    let (dir, target, planted) = planted_temp();

    let mut served = 0_usize;
    let used = create_temp(
        || {
            served += 1;
            if served == 1 {
                planted.clone()
            } else {
                dir.path().join(format!(".fresh-{served}.tmp"))
            }
        },
        &target,
        "id = \"connect-a1b2\"\n",
    )
    .expect("the retry must land on a fresh name");

    assert_ne!(used, planted, "the write must not use the planted temp");
    assert_eq!(
        std::fs::read_to_string(&used).expect("the fresh temp is written"),
        "id = \"connect-a1b2\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(&planted).expect("the plant survives"),
        "PLANT\n",
        "a pre-planted candidate is neither truncated into nor removed"
    );
}

/// The retry is BOUNDED: a name planted under every candidate fails
/// loud with `AlreadyExists` instead of silently downgrading to the
/// old truncating open.
#[test]
fn create_temp_fails_loud_when_every_candidate_is_planted() {
    let (_dir, target, planted) = planted_temp();

    let mut attempts = 0_usize;
    let err = create_temp(
        || {
            attempts += 1;
            planted.clone()
        },
        &target,
        "id = \"connect-a1b2\"\n",
    )
    .expect_err("a plant under every candidate must fail loud");

    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        attempts, TEMP_CREATE_ATTEMPTS,
        "the retry must be bounded, not unbounded"
    );
    assert_eq!(
        std::fs::read_to_string(&planted).expect("the plant survives"),
        "PLANT\n",
        "even the loud failure must not remove or truncate the plant"
    );
}

/// A temp left by a killed run (here a foreign pid, in the pre-fix
/// name shape) is a full copy of the operator's config: the next
/// successful write sweeps it best-effort, and no connect temp
/// survives the write.
#[test]
fn a_successful_write_sweeps_stale_connect_temps() {
    let (dir, path) = temp_config();
    let stale = dir.path().join(".config.toml.connect-999-abc.tmp");
    std::fs::write(&stale, b"leftover config copy\n").expect("seed a stale temp");
    let provider = test_provider("connect-a1b2");

    write_provider(&path, &provider).expect("write");

    assert!(!stale.exists(), "a stale sibling temp must be swept");
    let leftovers: Vec<String> =
        dir_file_names_matching(dir.path(), |name| name.starts_with(".config.toml.connect-"));
    assert!(
        leftovers.is_empty(),
        "no connect temp may survive a successful write: {leftovers:?}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .expect("read the written config")
            .contains("id = \"connect-a1b2\""),
        "the sweep must not have eaten the config itself"
    );
}

// ---------------------------------------------------------------------
// R4-1: two concurrent connects must never lose the loser's record
// (read-modify-write with no lock). The bytes are re-read after the
// splice and a drift refuses the write instead of renaming over the
// other writer's record.
// ---------------------------------------------------------------------

#[test]
fn config_drifted_is_a_byte_exact_comparison() {
    assert!(
        !config_drifted("a = 1\n", "a = 1\n"),
        "identical bytes never drift"
    );
    assert!(
        config_drifted("a = 1\n", "a = 2\n"),
        "different content is drift"
    );
    assert!(
        config_drifted("a = 1\n", "a = 1\n# otro escritor añadió una nota\n"),
        "a whitespace-only difference still means another writer landed"
    );
}

#[test]
fn ensure_config_unchanged_accepts_unchanged_bytes() {
    let (_dir, path) = temp_config();
    seed_config(&path, "format = \"markdown\"\n");
    let original = std::fs::read_to_string(&path).expect("read the snapshot");

    ensure_config_unchanged(&path, Some(&original))
        .expect("identical bytes must never block the write");
}

/// THE lost-update regression: while connect holds its snapshot, another
/// writer lands. The re-read differs, so the write refuses loud — the
/// alternative is renaming a stale splice over the other writer's
/// record while both printed success.
#[test]
fn ensure_config_unchanged_refuses_a_config_that_changed_on_disk() {
    let (_dir, path) = temp_config();
    seed_config(&path, "format = \"markdown\"\n");
    let original = std::fs::read_to_string(&path).expect("read the snapshot");
    std::fs::write(&path, "format = \"json\"\n# el otro connect\n")
        .expect("the other writer lands");

    let err = ensure_config_unchanged(&path, Some(&original))
        .expect_err("a config that changed on disk must refuse the write");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let message = err.to_string();
    assert!(
        message.contains("cambió en disco"),
        "the refusal must name the drift: {message}"
    );
    assert!(
        message.contains("connect de nuevo"),
        "the refusal must name the retry: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "format = \"json\"\n# el otro connect\n",
        "the refusal must leave the other writer's bytes untouched"
    );
}

/// The first-run race: the config was absent when connect read it and a
/// concurrent connect created it before the rename — renaming now would
/// clobber that record.
#[test]
fn ensure_config_unchanged_refuses_a_config_that_appeared_concurrently() {
    let (_dir, path) = temp_config();
    seed_config(&path, "format = \"markdown\"\n");

    let err = ensure_config_unchanged(&path, None)
        .expect_err("a config that appeared concurrently must refuse the write");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn ensure_config_unchanged_accepts_an_absent_config_that_stays_absent() {
    let (_dir, path) = temp_config();

    ensure_config_unchanged(&path, None)
        .expect("absent when read and absent now is a fresh first connect");
}

/// A config deleted between the read and the rename is drift too:
/// renaming into its place would silently replace whatever landed
/// there by then.
#[test]
fn ensure_config_unchanged_refuses_a_config_that_disappeared() {
    let (_dir, path) = temp_config();
    seed_config(&path, "format = \"markdown\"\n");
    let original = std::fs::read_to_string(&path).expect("read the snapshot");
    std::fs::remove_file(&path).expect("the other writer deletes it");

    let err = ensure_config_unchanged(&path, Some(&original))
        .expect_err("a config that disappeared must refuse the write");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// THE wiring regression: `write_provider` must REFUSE when the document
/// drifted, not merely own a drift-checker. Removing the
/// `ensure_config_unchanged` call from the commit path leaves every other
/// test green while the lost update ships — this test pins the ordering
/// itself. The drifted bytes are the OTHER writer's: they must survive
/// byte-for-byte, and no record of ours may be appended over them.
#[test]
fn commit_splice_refuses_to_rename_over_a_config_that_drifted() {
    let (_dir, path) = temp_config();
    seed_config(&path, "format = \"json\"\n# el otro connect gano\n");
    let stale_snapshot = "format = \"markdown\"\n".to_string();
    let spliced = stale_snapshot
        .parse::<toml_edit::DocumentMut>()
        .expect("a snapshot parses");

    let err = commit_splice(&path, &spliced, Some(&stale_snapshot))
        .expect_err("a drifted config must never be renamed over");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("connect de nuevo"),
        "the refusal must name the retry: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "format = \"json\"\n# el otro connect gano\n",
        "the other writer's config must survive the refusal untouched"
    );
}

/// The positive half of the wiring: an unchanged config commits, so the
/// refusal above cannot be satisfied by a commit path that always fails.
#[test]
fn commit_splice_writes_an_unchanged_config() {
    let (_dir, path) = temp_config();
    let original = "format = \"markdown\"\n";
    seed_config(&path, original);
    let spliced = original
        .parse::<toml_edit::DocumentMut>()
        .expect("a snapshot parses");

    commit_splice(&path, &spliced, Some(original)).expect("an unchanged config commits");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "format = \"markdown\"\n",
        "a no-drift commit writes exactly the spliced document"
    );
}
