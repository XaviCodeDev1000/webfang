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

/// Optional absolute path of the real browser (operator / test override).
const REAL_BINARY_ENV: &str = "WEBFANG_SANDBOX_REAL_BINARY";
/// Optional absolute path of `arapuca` (operator / test override).
const ARAPUCA_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
/// `on` / `off` override for the cgroup-delegation probe (`auto` otherwise).
const CGROUP_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
/// Per-run task identifier forwarded to `arapuca --task-id`.
const TASK_ENV: &str = "WEBFANG_RUN_ID";
const TENANT_ENV: &str = "WEBFANG_TENANT_ID";

/// Browser names tried in order when no explicit path is configured.
const BROWSER_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
];

/// Fixed mounts appended after the binary and profile directories. `/sys`
/// itself is never mounted — only the subtrees Chrome actually reads.
const FIXED_MOUNTS: &[(&str, &str)] = &[
    ("-v", "/proc:ro"),
    ("-v", "/dev/shm:rw"),
    ("-v", "/tmp:rw"),
    ("-v", "/sys/bus:ro"),
    ("-v", "/sys/class:ro"),
    ("-v", "/sys/devices:ro"),
    ("-v", "/etc/fonts:ro"),
    ("-v", "/usr/share/fonts:ro"),
    ("-v", "/dev/dri:rw"),
    ("-v", "/etc/ssl:ro"),
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
        warn_skip("arapuca binary unavailable");
        return exec(&browser, &args);
    };
    if !cgroup_delegation() {
        warn_skip("cgroup controllers not delegated");
        return exec(&browser, &args);
    }

    let argv = arapuca_args(&browser, profile.as_deref(), &task_id(), &args);
    let mut command = Command::new(&arapuca);
    command.args(&argv);
    let error = command.exec();
    eprintln!("webfang-sandbox-wrapper: arapuca exec failed: {error}");
    ExitCode::FAILURE
}

/// Emit a structured skip reason. WebFang's `tracing::WARN` counterpart lives
/// in the downloaders; this standalone binary has no subscriber installed.
fn warn_skip(reason: &str) {
    eprintln!("webfang-sandbox-wrapper: sandbox=skipped reason={reason}");
}

/// Replace the current process, propagating `exec` failures to the caller.
fn exec(binary: &Path, args: &[String]) -> ExitCode {
    let mut command = Command::new(binary);
    command.args(args);
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

/// Resolve `arapuca` from the explicit override, then from `PATH`.
fn resolve_arapuca() -> Option<PathBuf> {
    if let Some(path) = env::var_os(ARAPUCA_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    find_in_path("arapuca")
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Per-run task id: explicit run/tenant id, else this process's identity.
fn task_id() -> String {
    for key in [TASK_ENV, TENANT_ENV] {
        if let Some(value) = env::var(key).ok().filter(|v| !v.trim().is_empty()) {
            return value;
        }
    }
    format!("webfang-{}", std::process::id())
}

/// `on`/`off` override, otherwise the real cgroups-v2 delegation probe.
fn cgroup_delegation() -> bool {
    match env::var(CGROUP_ENV).as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => cgroup_delegated(),
    }
}

fn cgroup_delegated() -> bool {
    let Ok(contents) = fs::read_to_string("/proc/self/cgroup") else {
        return false;
    };
    let Some(relative) = contents.lines().find_map(|line| line.strip_prefix("0::")) else {
        return false;
    };
    let base = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
    fs::read_to_string(base.join("cgroup.controllers"))
        .is_ok_and(|controllers| !controllers.trim().is_empty())
}

/// Absolute directory containing `binary`, for the `:ro` binary mount.
fn binary_directory(binary: &Path) -> PathBuf {
    let parent = binary.parent().unwrap_or_else(|| Path::new("."));
    if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(parent)
    }
}

fn push_mount(argv: &mut Vec<String>, path: &Path, mode: &str) {
    argv.push("-v".to_string());
    argv.push(format!("{}:{mode}", path.display()));
}

/// Build the exact `arapuca run` argv mandated by the spec.
fn arapuca_args(
    binary: &Path,
    profile: Option<&Path>,
    task_id: &str,
    original: &[String],
) -> Vec<String> {
    let mut argv = vec![
        "run".to_string(),
        "--task-id".to_string(),
        task_id.to_string(),
        "--seccomp".to_string(),
        "baseline".to_string(),
    ];
    push_mount(&mut argv, &binary_directory(binary), "ro");
    if let Some(dir) = profile {
        push_mount(&mut argv, dir, "rw");
    }
    for (flag, spec) in FIXED_MOUNTS {
        argv.push((*flag).to_string());
        argv.push((*spec).to_string());
    }
    argv.push("--".to_string());
    argv.push(binary.to_string_lossy().into_owned());
    argv.extend(original.iter().cloned());
    argv
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
