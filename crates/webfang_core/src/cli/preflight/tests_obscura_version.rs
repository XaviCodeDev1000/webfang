use super::js_strategy::*;
use super::*;
use crate::domain::JsStrategy;

// ========================================================================
// #793 — Obscura minimum-version contract (parse / assess / gate)
// ========================================================================

/// `parse_obscura_version` extracts the first semver-like token from raw
/// `--version` output — pure, no process or env access.
#[test]
fn parse_obscura_version_extracts_semver_token() {
    assert_eq!(parse_obscura_version("obscura 0.2.0"), Some((0, 2, 0)));
    assert_eq!(parse_obscura_version("obscura 0.1.9"), Some((0, 1, 9)));
    assert_eq!(parse_obscura_version("0.10.3 (build 42)"), Some((0, 10, 3)));
    assert_eq!(parse_obscura_version("v1.2.3"), Some((1, 2, 3)));
}

/// Pre-release/build suffixes on the patch segment still parse; missing
/// versions, non-numeric segments, and empty output do not.
#[test]
fn parse_obscura_version_rejects_garbage() {
    assert_eq!(parse_obscura_version("obscura 0.2.0-rc.1"), Some((0, 2, 0)));
    assert_eq!(
        parse_obscura_version("obscura 0.2.0+build"),
        Some((0, 2, 0))
    );
    assert_eq!(parse_obscura_version("no version here"), None);
    assert_eq!(parse_obscura_version(""), None);
    assert_eq!(parse_obscura_version("version x.y.z"), None);
    assert_eq!(parse_obscura_version("obscura 0.2"), None);
}

/// `assess_obscura_version` classifies against the 0.2.5 minimum (#1710) —
/// the exact boundary, above it, below it, and the missing case.
#[test]
fn assess_obscura_version_classifies_meets_too_old_unknown() {
    // The floor itself is the boundary: 0.2.5 meets it.
    assert_eq!(
        assess_obscura_version(Some((0, 2, 5))),
        VersionVerdict::Meets
    );
    assert_eq!(
        assess_obscura_version(Some((0, 3, 0))),
        VersionVerdict::Meets
    );
    assert_eq!(
        assess_obscura_version(Some((1, 0, 0))),
        VersionVerdict::Meets
    );
    // One patch below the floor is too old.
    assert_eq!(
        assess_obscura_version(Some((0, 2, 4))),
        VersionVerdict::TooOld
    );
    // 0.2.0 met the old floor; since #1710 it is below it, because the
    // upstream security fixes it lacks are the reason the floor moved.
    assert_eq!(
        assess_obscura_version(Some((0, 2, 0))),
        VersionVerdict::TooOld
    );
    assert_eq!(
        assess_obscura_version(Some((0, 1, 9))),
        VersionVerdict::TooOld
    );
    assert_eq!(assess_obscura_version(None), VersionVerdict::Unknown);
}

/// Write an executable fake `obscura` whose `--version` prints
/// `obscura <version>` (#793). Deterministic: no network, no real binary.
#[cfg(unix)]
fn write_obscura_with_version(dir: &std::path::Path, version: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_path = dir.join("obscura");
    std::fs::write(
        &bin_path,
        format!("#!/bin/sh\necho \"obscura {version}\"\n"),
    )
    .expect("write fake obscura");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake obscura");
    bin_path
}

/// Hybrid + obscura 0.2.5: the version contract is met, preflight passes.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn hybrid_version_025_passes() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = write_obscura_with_version(tmp.path(), "0.2.5");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts).is_ok(),
        "obscura 0.2.5 must satisfy the version contract"
    );
}

/// Hybrid + obscura 0.1.9: config error (exit 78) naming the found
/// version, the required version, and both override surfaces.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn hybrid_version_below_minimum_errors() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = write_obscura_with_version(tmp.path(), "0.1.9");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
    let err = check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
        .expect_err("obscura below 0.2.5 must fail hybrid preflight");
    match err {
        CliExit::ConfigError(msg) => {
            assert!(
                msg.contains("0.1.9") && msg.contains(MINIMUM_OBSCURA_VERSION_STR),
                "config error must name found vs required version, got: {msg}"
            );
            assert!(
                msg.contains("--obscura-binary") && msg.contains("WEBFANG_OBSCURA_BINARY"),
                "config error must name the override surfaces, got: {msg}"
            );
        },
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// Fake `obscura` executable whose `--version` runs the given shell script,
/// wired into `CrawlOptions` under the Hybrid strategy: the common arrange
/// of the warn-degrade tests (#1710). The caller keeps the returned
/// `TempDir` bound while the options are in use — the fake binary lives
/// in it.
#[cfg(unix)]
fn hybrid_opts_with_version_script(script: &str) -> (tempfile::TempDir, CrawlOptions) {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("obscura");
    std::fs::write(&bin_path, script).expect("write fake obscura");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake obscura");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
    (tmp, opts)
}

/// Hybrid + obscura printing garbage on `--version`: warn-and-continue —
/// unknown builds are not hard-blocked.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn hybrid_version_garbage_degrades_to_warning() {
    let (_tmp, opts) = hybrid_opts_with_version_script("#!/bin/sh\necho \"custom build\"\n");
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts).is_ok(),
        "an unparseable --version must degrade to a warning, not block"
    );
}

/// Hybrid + obscura exiting non-zero on `--version`: same warn-degrade.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn hybrid_version_probe_failure_degrades_to_warning() {
    let (_tmp, opts) = hybrid_opts_with_version_script("#!/bin/sh\nexit 3\n");
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts).is_ok(),
        "a failing --version probe must degrade to a warning, not block"
    );
}

/// #1710 — the `Unknown` verdict must NAME the expected minimum.
///
/// A verdict that only says "the version could not be determined" is one
/// the operator cannot act on: the floor the guard enforces
/// ([`MINIMUM_OBSCURA_VERSION_STR`]) is the whole point of the check, so
/// the warning has to state it. The verdict itself does NOT change: it
/// stays `Unknown`, it stays a DEGRADATION (the preflight returns `Ok`),
/// and the crawl and the release flow keep going.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn unknown_verdict_names_the_expected_minimum() {
    use crate::infrastructure::observability::test_log_capture::LogBuffer;

    let (_tmp, opts) = hybrid_opts_with_version_script("#!/bin/sh\necho \"custom build\"\n");

    let mut verdict = None;
    let out = LogBuffer::run(|| {
        verdict = Some(check_js_dependencies_with(
            &["definitely-not-installed-9x4k"],
            false,
            "",
            &opts,
        ));
    })
    .text();

    let verdict = verdict.expect("the preflight must produce a verdict");
    assert!(
        verdict.is_ok(),
        "the Unknown verdict must stay a degradation, not an error: {verdict:?}"
    );
    assert!(
        out.contains(MINIMUM_OBSCURA_VERSION_STR),
        "the warning must name the expected minimum ({MINIMUM_OBSCURA_VERSION_STR}), \
         got: {out}"
    );
}
