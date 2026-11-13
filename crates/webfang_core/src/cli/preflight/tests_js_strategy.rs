use super::*;

// ========================================================================
// #685 — --js-strategy full preflight Chrome dependency check
// ========================================================================

/// `Static` strategy never needs Chrome: the check must return `Ok`
/// without probing any binary, so the injected candidate list is
/// irrelevant.
#[test]
fn static_strategy_ok_without_spawn() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Static;
    // A nonexistent candidate proves the check short-circuits before it
    // could attempt to spawn anything for a non-Full strategy.
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts).is_ok(),
        "Static strategy must not require Chrome or spawn processes"
    );
}

/// `Static` strategy must not validate the Obscura binary at all (#787):
/// an unresolvable `--obscura-binary` is a no-op for static crawls.
#[test]
fn static_strategy_ignores_missing_obscura_binary() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Static;
    opts.network.obscura_binary = "/definitely/not/here/obscura".to_string();
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts).is_ok(),
        "Static strategy must not check --obscura-binary"
    );
}

/// `Hybrid` strategy escalates through Obscura (Layer 2), which works
/// without the `chromium` feature — the preflight must not gate it
/// (#758). The Obscura binary itself is supplied through a controlled
/// PATH pointing at a tempdir with a fake `obscura` file, so this test
/// stays free of process-global env mutation.
// The fake obscura binary is a `#!/bin/sh` script resolved through an
// extension-less PATH lookup — unix semantics with no Windows equivalent
// to assert (#1825).
#[cfg(unix)]
#[test]
fn hybrid_strategy_ok_without_chromium_feature() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("obscura");
    std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    let path_value = tmp.path().to_string_lossy();
    assert!(
        check_js_dependencies_with(
            &["definitely-not-installed-9x4k"],
            false,
            &path_value,
            &opts
        )
        .is_ok(),
        "Hybrid strategy must not require the chromium feature or a Chrome binary"
    );
}

/// `Full` strategy on a binary built WITHOUT the `chromium` feature
/// fails fast with a config error naming the feature — before any PATH
/// probe (#758).
#[test]
fn full_strategy_errors_without_chromium_feature() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Full;
    // `true` exists in CI, so a passing candidate list proves the
    // feature gate fires BEFORE the binary probe.
    let err = check_js_dependencies_with(&["true"], false, "", &opts)
        .expect_err("feature gate must fail before probing binaries");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("chromium"),
            "config error must name the chromium feature, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// `Full` strategy with a present, exit-0 binary passes the preflight
/// check. Hermetic: a fake `google-chrome` executable in a controlled
/// PATH dir (same pattern as the obscura fakes), so the test never
/// depends on the real process environment.
// Miri cannot emulate posix_spawn (Command::status), so the whole test tree
// here is excluded (#775): no text marker can tell a test that spawns a
// resolved binary from one that only names a missing one.
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn full_strategy_ok_when_binary_reports_version() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    write_chrome_like(tmp.path(), "google-chrome", 0);
    let path_value = tmp.path().to_string_lossy().into_owned();

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Full;
    assert!(
        check_js_dependencies_with(&["google-chrome"], true, &path_value, &opts).is_ok(),
        "a candidate that exits 0 must satisfy the Full-strategy check"
    );
}

/// F-52-c (#1278): the resolver returns the first candidate (in order)
/// that exists as a file AND reports a version — gate-certified is
/// exactly what the launcher will execute.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn resolve_chrome_binary_prefers_first_working_candidate() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    write_chrome_like(tmp.path(), "google-chrome", 0);
    write_chrome_like(tmp.path(), "chromium", 0);
    let path_value = tmp.path().to_string_lossy().into_owned();

    let resolved = resolve_chrome_binary_with(&["google-chrome", "chromium"], &path_value)
        .expect("a working candidate must resolve");
    assert_eq!(resolved, tmp.path().join("google-chrome"));
}

/// F-52-c (#1278): a *directory* named like a candidate must be skipped
/// (`is_file`, not `exists`) — this is the `/opt/google/chrome` trap
/// where chromiumoxide's `get_by_path` accepted a directory and the
/// launch died with permission denied.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn resolve_chrome_binary_skips_directories() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir(tmp.path().join("google-chrome")).expect("fake chrome directory");
    write_chrome_like(tmp.path(), "chromium", 0);
    let path_value = tmp.path().to_string_lossy().into_owned();

    let resolved = resolve_chrome_binary_with(&["google-chrome", "chromium"], &path_value)
        .expect("the working fallback must resolve");
    assert_eq!(resolved, tmp.path().join("chromium"));
}

/// F-52-c (#1278): a candidate whose `--version` probe fails is skipped,
/// and no resolvable candidate is `None` (gate goes red downstream).
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn resolve_chrome_binary_skips_failing_version_probe() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    write_chrome_like(tmp.path(), "google-chrome", 1);
    let path_value = tmp.path().to_string_lossy().into_owned();

    assert!(
        resolve_chrome_binary_with(&["google-chrome"], &path_value).is_none(),
        "a candidate that fails --version must not resolve"
    );
    assert!(
        resolve_chrome_binary_with(&["definitely-not-installed-9x4k"], &path_value).is_none(),
        "a missing candidate must not resolve"
    );
}

/// Write a fake Chrome-like executable: exits `code` on `--version`.
/// Mirrors `write_obscura_with_version` (#793 pattern).
#[cfg(unix)]
fn write_chrome_like(dir: &std::path::Path, name: &str, code: i32) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_path = dir.join(name);
    std::fs::write(&bin_path, format!("#!/bin/sh\nexit {code}\n"))
        .expect("write fake chrome binary");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake chrome binary");
    bin_path
}

/// `Full` strategy with no working candidate yields a config error whose
/// message names Chrome (user-facing, Spanish).
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[test]
fn full_strategy_errors_when_no_binary_found() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Full;
    let err = check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts)
        .expect_err("no candidate binary present means the check must fail");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("Chrome"),
            "config error must name Chrome, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}
