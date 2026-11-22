use crate::bin_path::{prebuilt_bin_declared, resolve_webfang_path, workspace_root_from_manifest};
use std::path::{Path, PathBuf};

/// The expected binary leaf name is platform-dependent: Windows builds
/// `webfang.exe`. Mirrors `resolve_webfang_path`'s own
/// `set_extension("exe")` so the expected constants stay cross-platform.
fn expected_binary_name() -> &'static str {
    if cfg!(windows) {
        "webfang.exe"
    } else {
        "webfang"
    }
}

/// #1366 regression pin: with BOTH harness variables absent
/// (`CARGO_BIN_EXE_webfang` unset — the binary belongs to a sibling crate —
/// and `CARGO_TARGET_DIR` unset — a runner without direnv), the fallback
/// must resolve the binary under the workspace root's own `target/`,
/// reached by climbing exactly TWO parent hops from the crate manifest.
/// The bug climbed three, landing one directory too high (the repo's
/// parent), so the fallback path pointed at a `target/` directory cargo
/// never populates — the build succeeded and the returned path was still
/// wrong. This test simulates the absent-variable environment by passing
/// `None` explicitly: it never mutates (or even reads) the process
/// environment, so it cannot race ENV_LOCK or leak state into siblings.
#[test]
fn resolve_webfang_path_without_both_env_vars_uses_workspace_target() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = resolve_webfang_path(None, None, manifest);

    let root = workspace_root_from_manifest(manifest);
    assert_eq!(
        path,
        root.join("target")
            .join("debug")
            .join(expected_binary_name())
    );

    // The two-hop root must be the real workspace root: it holds the
    // virtual manifest, and it is NOT the three-hop directory the #1366
    // bug used to land in (the repo's parent).
    assert!(root.join("Cargo.toml").is_file());
    let buggy_three_hop_root = Path::new(manifest)
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("three-hop chain resolves");
    assert_ne!(root, buggy_three_hop_root);
}

/// Cross-check the two-hop climb against the authoritative source of
/// truth: `cargo locate-project --workspace` reports exactly where the
/// workspace manifest lives. #1366 regressed here because the climb was
/// hand-rolled; this pin fails if the climb and cargo ever disagree again.
#[test]
fn workspace_root_from_manifest_matches_cargo_metadata() {
    let root = workspace_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .args([
            "locate-project",
            "--workspace",
            "--message-format=plain",
            "--manifest-path",
        ])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .expect("spawn cargo locate-project");
    assert!(out.status.success(), "cargo locate-project failed");
    // locate-project prints the workspace manifest path; strip the
    // `Cargo.toml` leaf to compare roots.
    let manifest_out = String::from_utf8(out.stdout).expect("locate-project prints UTF-8");
    let cargo_root = Path::new(manifest_out.trim_end())
        .parent()
        .expect("locate-project prints a manifest path");
    assert_eq!(
        root, cargo_root,
        "two-hop climb must land on cargo's own workspace root"
    );
}

/// Behavioral pins for the variable-present branches — the paths that
/// must NOT change with #1366: an explicit `CARGO_BIN_EXE_webfang` wins
/// outright, and an explicit `CARGO_TARGET_DIR` roots the debug output.
#[test]
fn resolve_webfang_path_respects_explicit_env_vars() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    assert_eq!(
        resolve_webfang_path(Some("/explicit/webfang"), None, manifest),
        PathBuf::from("/explicit/webfang")
    );

    let via_target = resolve_webfang_path(None, Some("/custom/target"), manifest);
    assert_eq!(
        via_target,
        PathBuf::from("/custom/target/debug").join(expected_binary_name())
    );
}

/// The runner contract is an exact marker: only `"1"` declares a
/// pre-built binary. Any other value — including an empty string, which a
/// sloppy workflow could interpolate — keeps the local on-demand build, so
/// a typo never turns into a silent stale-binary reuse (#2331).
#[test]
fn only_the_exact_marker_declares_a_prebuilt_binary() {
    assert!(prebuilt_bin_declared(Some("1")));
    assert!(!prebuilt_bin_declared(Some("0")));
    assert!(!prebuilt_bin_declared(Some("true")));
    assert!(!prebuilt_bin_declared(Some("")));
    assert!(!prebuilt_bin_declared(None));
}
