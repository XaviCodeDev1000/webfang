//! Observable behaviour of the sandbox wrapper binary.
//!
//! Both tests use ephemeral shell fixtures (no real browser, no real
//! arapuca, no cgroup access) and exercise the compiled binary through its
//! process boundary. Environment overrides are restored by `EnvGuard`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

const WRAPPER: &str = env!("CARGO_BIN_EXE_webfang-sandbox-wrapper");
const REAL_BINARY_ENV: &str = "WEBFANG_SANDBOX_REAL_BINARY";
const ARAPUCA_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
const CGROUP_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
const TASK_ENV: &str = "WEBFANG_RUN_ID";

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fixture script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fixture script");
    path
}

fn guard(vars: &[(&str, &str)]) -> webfang_test_utils::EnvGuard {
    webfang_test_utils::EnvGuard::with(vars)
}

#[test]
fn version_is_forwarded_to_the_real_browser() {
    let tmp = TempDir::new().expect("tempdir");
    let browser = script(tmp.path(), "browser", "printf 'fake-browser 1.2.3\\n'");
    let _env = guard(&[(REAL_BINARY_ENV, browser.to_str().expect("utf8 path"))]);

    let output = Command::new(WRAPPER)
        .arg("--version")
        .output()
        .expect("run wrapper");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "fake-browser 1.2.3\n"
    );
}

#[test]
fn user_data_dir_is_created_and_original_argv_reach_the_browser() {
    let tmp = TempDir::new().expect("tempdir");
    let browser = script(
        tmp.path(),
        "browser",
        "for a in \"$@\"; do printf '%s\\n' \"$a\"; done",
    );
    let arapuca = script(
        tmp.path(),
        "arapuca",
        "while [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"",
    );
    let profile = tmp.path().join("profile").join("nested");
    let profile_arg = format!("--user-data-dir={}", profile.display());
    let _env = guard(&[
        (REAL_BINARY_ENV, browser.to_str().expect("utf8 path")),
        (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
        (CGROUP_ENV, "on"),
        (TASK_ENV, "tenant-42"),
    ]);

    let output = Command::new(WRAPPER)
        .arg(&profile_arg)
        .arg("--headless")
        .output()
        .expect("run wrapper");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        profile.is_dir(),
        "wrapper must create --user-data-dir when absent"
    );
    let forwarded = String::from_utf8_lossy(&output.stdout);
    assert!(
        forwarded.lines().any(|a| a == profile_arg),
        "original argv lost: {forwarded}"
    );
    assert!(
        forwarded.lines().any(|a| a == "--headless"),
        "original argv lost: {forwarded}"
    );
}

#[test]
fn real_binary_override_is_stripped_before_browser_exec() {
    let tmp = TempDir::new().expect("tempdir");
    let browser = script(
        tmp.path(),
        "browser",
        "printf '%s' \"${WEBFANG_SANDBOX_REAL_BINARY-unset}\"",
    );
    let _env = guard(&[(REAL_BINARY_ENV, browser.to_str().expect("utf8 path"))]);
    let output = Command::new(WRAPPER)
        .arg("--version")
        .output()
        .expect("run wrapper");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "unset");
}
