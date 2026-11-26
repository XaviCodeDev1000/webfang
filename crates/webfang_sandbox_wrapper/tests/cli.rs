//! Observable behaviour of the sandbox wrapper binary.
//!
//! Both tests use ephemeral shell fixtures (no real browser, no real
//! arapuca, no cgroup access) and exercise the compiled binary through its
//! process boundary. Environment overrides are restored by `EnvGuard`.
//!
//! The wrapper is Linux-only: on every other platform it exits with a
//! diagnostic, so this target compiles empty there instead of failing the
//! workspace `--tests` run.

#![cfg(target_os = "linux")]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

const WRAPPER: &str = env!("CARGO_BIN_EXE_webfang-sandbox-wrapper");
const REAL_BINARY_ENV: &str = "WEBFANG_SANDBOX_REAL_BINARY";
const ARAPUCA_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
const CGROUP_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
const TASK_ENV: &str = "WEBFANG_RUN_ID";

fn script(dir: &Path, name: &str, body: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn guard(vars: &[(&str, &str)]) -> webfang_test_utils::EnvGuard {
    webfang_test_utils::EnvGuard::with(vars)
}

/// A fake browser plus the env overrides pointing the wrapper at it. The
/// fixture owns the `TempDir` and the `EnvGuard` so both stay alive until the
/// test has inspected the child's output; the guard restores the overrides on
/// drop. The command is ready for the test to append its argv.
struct BrowserFixture {
    _tmp: TempDir,
    _env: webfang_test_utils::EnvGuard,
    command: Command,
}

fn browser_fixture(body: &str) -> io::Result<BrowserFixture> {
    let tmp = TempDir::new()?;
    let browser = script(tmp.path(), "browser", body)?;
    let real = browser.to_string_lossy();
    let env = guard(&[(REAL_BINARY_ENV, real.as_ref())]);
    Ok(BrowserFixture {
        _tmp: tmp,
        _env: env,
        command: Command::new(WRAPPER),
    })
}

/// Run `--version` on the fixture and pin both the exit status and the exact
/// stdout the fake browser reports.
fn assert_version(fixture: &mut BrowserFixture, expected: &str) -> io::Result<()> {
    let output = fixture.command.arg("--version").output()?;
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    Ok(())
}

#[test]
fn version_is_forwarded_to_the_real_browser() -> io::Result<()> {
    let mut fixture = browser_fixture("printf 'fake-browser 1.2.3\\n'")?;
    assert_version(&mut fixture, "fake-browser 1.2.3\n")
}

#[test]
fn user_data_dir_is_created_and_original_argv_reach_the_browser() -> io::Result<()> {
    let tmp = TempDir::new()?;
    let browser = script(
        tmp.path(),
        "browser",
        "for a in \"$@\"; do printf '%s\\n' \"$a\"; done",
    )?;
    let arapuca = script(
        tmp.path(),
        "arapuca",
        "while [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"",
    )?;
    let profile = tmp.path().join("profile").join("nested");
    let profile_arg = format!("--user-data-dir={}", profile.display());
    let real = browser.to_string_lossy();
    let arapuca_path = arapuca.to_string_lossy();
    let _env = guard(&[
        (REAL_BINARY_ENV, real.as_ref()),
        (ARAPUCA_ENV, arapuca_path.as_ref()),
        (CGROUP_ENV, "on"),
        (TASK_ENV, "tenant-42"),
    ]);

    let output = Command::new(WRAPPER)
        .arg(&profile_arg)
        .arg("--headless")
        .output()?;

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
    Ok(())
}

#[test]
fn real_binary_override_is_stripped_before_browser_exec() -> io::Result<()> {
    let mut fixture = browser_fixture("printf '%s' \"${WEBFANG_SANDBOX_REAL_BINARY-unset}\"")?;
    assert_version(&mut fixture, "unset")
}
