use super::*;

// ========================================================================
// #787 — --js-strategy hybrid obscura binary preflight
// ========================================================================

/// `has_path_separator` distinguishes explicit paths from bare names.
#[test]
fn has_path_separator_detects_explicit_paths() {
    assert!(has_path_separator("/usr/local/bin/obscura"));
    assert!(has_path_separator("./obscura"));
    assert!(has_path_separator("bin/obscura"));
    assert!(has_path_separator("C:\\tools\\obscura.exe"));
    assert!(!has_path_separator("obscura"));
    assert!(!has_path_separator(""));
}

/// `resolve_executable_in_path` scans PATH entries in order and returns
/// the first existing file with the given name — a pure function over an
/// injected PATH value, no process-global state.
#[test]
fn resolve_executable_in_path_finds_file_in_path_entries() {
    let empty = tempfile::TempDir::new().expect("tempdir");
    let with_bin = tempfile::TempDir::new().expect("tempdir");
    let bin_path = with_bin.path().join("obscura");
    std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

    // Found in the second entry.
    let joined = std::env::join_paths([empty.path(), with_bin.path()]).expect("join_paths");
    let path_value = joined.to_string_lossy();
    let resolved = resolve_executable_in_path("obscura", &path_value)
        .expect("obscura must resolve through the injected PATH");
    assert_eq!(resolved, bin_path);

    // Empty PATH resolves nothing.
    assert!(resolve_executable_in_path("obscura", "").is_none());
    // Explicit names with separators are rejected.
    assert!(resolve_executable_in_path("./obscura", &path_value).is_none());
    // A directory named `obscura` is not an executable file.
    let dir_only = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir(dir_only.path().join("obscura")).expect("mkdir");
    let dir_path = dir_only.path().to_string_lossy();
    assert!(resolve_executable_in_path("obscura", &dir_path).is_none());
}

/// Hybrid + nonexistent absolute path: config error naming the flag and
/// the env var (#787). No spawn — pure filesystem lookup.
#[test]
fn hybrid_nonexistent_absolute_path_errors() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = "/definitely/not/here/obscura".to_string();
    let err = check_js_dependencies_with(&["true"], true, "", &opts)
        .expect_err("a missing obscura path must fail hybrid preflight");
    match err {
        CliExit::ConfigError(msg) => {
            assert!(
                msg.contains("--obscura-binary") && msg.contains("WEBFANG_OBSCURA_BINARY"),
                "config error must name the flag and the env var, got: {msg}"
            );
            assert!(
                msg.contains("/definitely/not/here/obscura"),
                "config error must name the offending path, got: {msg}"
            );
        },
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// Hybrid + nonexistent relative path: same config-error shape as the
/// absolute case.
#[test]
fn hybrid_nonexistent_relative_path_errors() {
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = "definitely/not/here/obscura".to_string();
    let err = check_js_dependencies_with(&["true"], true, "", &opts)
        .expect_err("a missing relative obscura path must fail hybrid preflight");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--obscura-binary"),
            "config error must name the flag, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// Hybrid + bare `obscura` with PATH pointing at an empty directory:
/// the binary cannot be on PATH, so the check must fail fast.
#[test]
fn hybrid_bare_name_missing_from_path_errors() {
    let empty = tempfile::TempDir::new().expect("tempdir");
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = "obscura".to_string();
    let path_value = empty.path().to_string_lossy();
    let err = check_js_dependencies_with(&["true"], true, &path_value, &opts)
        .expect_err("obscura missing from PATH must fail hybrid preflight");
    match err {
        CliExit::ConfigError(msg) => {
            assert!(
                msg.contains("PATH")
                    && msg.contains("--obscura-binary")
                    && msg.contains("WEBFANG_OBSCURA_BINARY"),
                "config error must name PATH, the flag and the env var, got: {msg}"
            );
            assert!(
                msg.contains("\"obscura\""),
                "config error must name the missing binary, got: {msg}"
            );
        },
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// Hybrid + a fake `obscura` file on the injected PATH: the check
/// passes with no `chromium` feature required.
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[test]
fn hybrid_binary_found_on_path_ok() {
    let bin_dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(bin_dir.path().join("obscura"), "#!/bin/sh\n").expect("write fake binary");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = "obscura".to_string();
    let path_value = bin_dir.path().to_string_lossy();
    assert!(
        check_js_dependencies_with(
            &["definitely-not-installed-9x4k"],
            false,
            &path_value,
            &opts
        )
        .is_ok(),
        "an obscura binary on PATH must satisfy hybrid preflight"
    );
}

/// Hybrid + explicit path to an existing file: the check passes even
/// with an empty PATH (paths never fall back to PATH lookup).
#[test]
fn hybrid_explicit_existing_path_ok_with_empty_path() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("obscura");
    std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    // Absolute path to the fake binary created above.
    opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
    assert!(
        check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts).is_ok(),
        "an existing explicit obscura path must satisfy hybrid preflight"
    );
}
