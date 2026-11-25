//! Linux sandbox wrapper for WebFang's browser subprocesses.
//!
//! Chromiumoxide launches this binary instead of the real browser. On a
//! normal invocation the wrapper reads Chrome's `--user-data-dir=<path>`,
//! ensures that directory exists, and replaces itself with
//! `arapuca run ... -- <real-browser> <original argv>` so the browser runs
//! under the baseline seccomp profile. `--version` is forwarded straight to
//! the real browser (no sandbox, no profile creation) because launchers use
//! it as a capability probe.
//!
//! Every unavailable sandbox prerequisite (non-Linux, missing cgroup
//! delegation, missing `arapuca`) degrades to a direct browser exec: a
//! missing sandbox must never fail a run. All APIs used here are safe.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use webfang_core::infrastructure::downloader::obscura_downloader::sandbox::{
    arapuca_args, cgroup_delegation, find_in_path, resolve_arapuca, task_id, REAL_BINARY_ENV,
};

/// Browser names tried in order when no explicit path is configured.
const BROWSER_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
];

fn main() -> ExitCode {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("webfang-sandbox-wrapper: sandbox is Linux-only");
        ExitCode::FAILURE
    }
    #[cfg(target_os = "linux")]
    run()
}

#[cfg(target_os = "linux")]
fn run() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    // Capability probes must reach the real browser unmodified.
    if args.iter().any(|arg| arg == "--version") {
        let Some(browser) = resolve_real_browser() else {
            eprintln!("webfang-sandbox-wrapper: real browser not found for --version");
            return ExitCode::FAILURE;
        };
        return exec(&browser, &args);
    }

    let profile = parse_user_data_dir(&args);
    if let Some(dir) = &profile {
        if let Err(error) = fs::create_dir_all(dir) {
            eprintln!(
                "webfang-sandbox-wrapper: cannot create profile dir {}: {error}",
                dir.display()
            );
            return ExitCode::FAILURE;
        }
    }

    let Some(browser) = resolve_real_browser() else {
        eprintln!("webfang-sandbox-wrapper: real browser not found");
        return ExitCode::FAILURE;
    };

    let Some(arapuca) = resolve_arapuca() else {
        warn_skip("arapuca binary unavailable", &task_id());
        return exec(&browser, &args);
    };
    if !cgroup_delegation() {
        warn_skip("cgroup controllers not delegated", &task_id());
        return exec(&browser, &args);
    }

    let argv = arapuca_args(&browser, profile.as_deref(), &task_id(), &args);
    let mut command = Command::new(&arapuca);
    command.args(&argv);
    // arapuca execs the browser: the override is consumed here, not inherited.
    command.env_remove(REAL_BINARY_ENV);
    let error = command.exec();
    eprintln!("webfang-sandbox-wrapper: arapuca exec failed: {error}");
    ExitCode::FAILURE
}

/// Emit a structured skip reason. WebFang's `tracing::WARN` counterpart lives
/// in the downloaders; this standalone binary has no subscriber installed, so
/// it carries the same run id in its stderr line and the parent process owns
/// the FileTraceLayer event.
fn warn_skip(reason: &str, task_id: &str) {
    eprintln!("webfang-sandbox-wrapper: sandbox=skipped layer=3 reason={reason} task_id={task_id}");
}

/// Replace the current process, propagating `exec` failures to the caller.
fn exec(binary: &Path, args: &[String]) -> ExitCode {
    let mut command = Command::new(binary);
    command.args(args);
    // The override selects the real browser; Chrome must never inherit it.
    command.env_remove(REAL_BINARY_ENV);
    let error = command.exec();
    eprintln!(
        "webfang-sandbox-wrapper: exec {}: {error}",
        binary.display()
    );
    ExitCode::FAILURE
}

/// Extract `--user-data-dir=<path>` from the browser's argv.
fn parse_user_data_dir(args: &[String]) -> Option<PathBuf> {
    let raw = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--user-data-dir="))?;
    (!raw.is_empty()).then(|| PathBuf::from(raw))
}

/// Resolve the real browser from the explicit override, then from `PATH`.
fn resolve_real_browser() -> Option<PathBuf> {
    if let Some(path) = env::var_os(REAL_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    BROWSER_CANDIDATES
        .iter()
        .find_map(|name| find_in_path(name))
}

#[cfg(test)]
mod tests {
    use super::{arapuca_args, parse_user_data_dir};

    #[test]
    fn parse_user_data_dir_accepts_only_the_explicit_form() {
        let args = vec![
            "--headless".to_string(),
            "--user-data-dir=/tmp/webfang-profile".to_string(),
        ];
        assert_eq!(
            parse_user_data_dir(&args),
            Some(std::path::PathBuf::from("/tmp/webfang-profile"))
        );
        assert_eq!(parse_user_data_dir(&["--user-data-dir=".to_string()]), None);
        assert_eq!(parse_user_data_dir(&["--headless".to_string()]), None);
    }

    #[test]
    fn arapuca_args_follow_the_exact_spec_order() {
        let binary = std::path::Path::new("/opt/google/chrome/chrome");
        let profile = std::path::Path::new("/tmp/webfang-chrome-1");
        let original = vec!["--headless".to_string(), "--no-sandbox".to_string()];
        assert_eq!(
            arapuca_args(binary, Some(profile), "tenant-9", &original),
            vec![
                "run",
                "--task-id",
                "tenant-9",
                "--seccomp",
                "baseline",
                "-v",
                "/opt/google/chrome:ro",
                "-v",
                "/tmp/webfang-chrome-1:rw",
                "-v",
                "/proc:ro",
                "-v",
                "/dev/shm:rw",
                "-v",
                "/tmp:rw",
                "-v",
                "/sys/bus:ro",
                "-v",
                "/sys/class:ro",
                "-v",
                "/sys/devices:ro",
                "-v",
                "/etc/fonts:ro",
                "-v",
                "/usr/share/fonts:ro",
                "-v",
                "/dev/dri:rw",
                "-v",
                "/etc/ssl:ro",
                "--",
                "/opt/google/chrome/chrome",
                "--headless",
                "--no-sandbox",
            ]
        );
    }

    #[test]
    fn arapuca_args_omit_profile_mount_when_unresolved() {
        let argv = arapuca_args(
            std::path::Path::new("/usr/bin/obscura"),
            None,
            "webfang-1",
            &[],
        );
        assert!(!argv.iter().any(|arg| arg == ":rw"));
        assert_eq!(
            &argv[..5],
            &["run", "--task-id", "webfang-1", "--seccomp", "baseline"]
        );
    }
}
