//! Shared sandbox resolution for the browser downloader layers (S3).
//!
//! No global registry: the task id comes from the run's own identity (an
//! explicit run/tenant id, else this process) and the binaries come from an
//! explicit override or `PATH`. Both L2 (Obscura) and L3 (Chromiumoxide)
//! consume these helpers, so the mount order exists in exactly one place.
//!
//! Every missing prerequisite is a reason to skip the sandbox, never a
//! reason to fail the fetch.

use std::env;
use std::path::{Path, PathBuf};

/// Explicit path to `arapuca`, used when the binary is not on `PATH`.
pub const ARAPUCA_BINARY_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
/// Explicit run identifier forwarded to `arapuca --task-id`.
pub const RUN_ID_ENV: &str = "WEBFANG_RUN_ID";
/// Explicit tenant identifier, used when no run id is set.
pub const TENANT_ID_ENV: &str = "WEBFANG_TENANT_ID";
/// `on`/`off` override for the cgroup-delegation probe (`auto` otherwise).
pub const CGROUP_DELEGATION_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
/// Real-browser override consumed by the sandbox wrapper before exec.
pub const REAL_BINARY_ENV: &str = "WEBFANG_SANDBOX_REAL_BINARY";
/// Explicit override for the sandbox wrapper binary.
#[cfg(feature = "chromium")]
const SANDBOX_WRAPPER_ENV: &str = "WEBFANG_SANDBOX_WRAPPER";
/// Name of the sibling/PATH wrapper binary built by this workspace.
#[cfg(feature = "chromium")]
const SANDBOX_WRAPPER_NAME: &str = "webfang-sandbox-wrapper";

/// Mounts appended after the binary and profile directories. `/sys` itself
/// is never mounted — Chrome only needs these subtrees.
pub const FIXED_MOUNTS: &[(&str, &str)] = &[
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

/// Sandbox prerequisites resolved once per downloader build.
#[derive(Clone, Debug)]
pub(crate) struct SandboxConfig {
    /// Resolved `arapuca` binary, if present.
    pub(crate) arapuca: Option<PathBuf>,
    /// Per-run identifier forwarded via `--task-id`.
    pub(crate) task_id: String,
    /// Whether cgroups v2 exposes delegated controllers.
    pub(crate) cgroup_delegated: bool,
}

impl SandboxConfig {
    /// Resolve the sandbox prerequisites from the current environment.
    pub(crate) fn resolve() -> Self {
        Self {
            arapuca: resolve_arapuca(),
            task_id: task_id(),
            cgroup_delegated: cgroup_delegation(),
        }
    }

    /// Why the sandbox cannot be applied, or `None` when it can.
    pub(crate) fn skip_reason(&self) -> Option<&'static str> {
        if !cfg!(target_os = "linux") {
            Some("non-linux platform")
        } else if self.arapuca.is_none() {
            Some("arapuca binary not found")
        } else if !self.cgroup_delegated {
            Some("cgroup controllers not delegated")
        } else {
            None
        }
    }
}

/// Resolve `arapuca` from the explicit override, then from `PATH`.
pub fn resolve_arapuca() -> Option<PathBuf> {
    if let Some(path) = env::var_os(ARAPUCA_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    find_in_path("arapuca")
}

/// Per-run task id: explicit run/tenant id, else this process's identity.
pub fn task_id() -> String {
    for key in [RUN_ID_ENV, TENANT_ID_ENV] {
        if let Some(value) = env::var(key).ok().filter(|v| !v.trim().is_empty()) {
            return value;
        }
    }
    format!("webfang-{}", std::process::id())
}

/// `on`/`off` override, otherwise the real cgroups-v2 delegation probe.
pub fn cgroup_delegation() -> bool {
    match env::var(CGROUP_DELEGATION_ENV).as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => cgroup_delegated(),
    }
}

/// Probe cgroups v2 for delegated controllers.
pub fn cgroup_delegated() -> bool {
    let Ok(contents) = std::fs::read_to_string("/proc/self/cgroup") else {
        return false;
    };
    let Some(relative) = contents.lines().find_map(|line| line.strip_prefix("0::")) else {
        return false;
    };
    let base = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
    std::fs::read_to_string(base.join("cgroup.controllers"))
        .is_ok_and(|controllers| !controllers.trim().is_empty())
}

/// Find an executable by name on `PATH`.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Resolve the executable a [`Path`] names, for the `:ro` binary mount.
///
/// A bare name (`obscura`, the default) has no parent —
/// `Path::parent()` returns an empty path — so mounting `binary_directory`
/// would mount the current directory and exec the name from a directory that
/// does not contain it. Resolve it on `PATH` exactly like the OS would on
/// spawn; `None` when a bare name is not on `PATH`, which makes the caller
/// skip the sandbox instead of mounting an unrelated directory.
pub fn resolve_executable(binary: &Path) -> Option<PathBuf> {
    if binary.is_absolute() {
        return Some(binary.to_path_buf());
    }
    if binary.components().count() > 1 {
        return std::path::absolute(binary).ok();
    }
    find_in_path(&binary.to_string_lossy())
}

/// Absolute directory containing `binary`, for the `:ro` binary mount.
pub fn binary_directory(binary: &Path) -> PathBuf {
    let parent = binary.parent().unwrap_or_else(|| Path::new("."));
    if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(parent)
    }
}

/// Append a `-v <path>:<mode>` mount argument to `argv`.
pub fn push_mount(argv: &mut Vec<String>, path: &Path, mode: &str) {
    argv.push("-v".to_string());
    argv.push(format!("{}:{mode}", path.display()));
}

/// `arapuca run` argv in the exact spec order; `profile` adds the optional rw mount.
pub fn arapuca_args(
    binary: &Path,
    profile: Option<&Path>,
    task_id: &str,
    inner: &[String],
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
    argv.extend(inner.iter().cloned());
    argv
}

/// Resolve the wrapper binary without mutating any global registry: explicit
/// override, then next to the running executable, then `PATH`.
#[cfg(feature = "chromium")]
fn resolve_wrapper_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os(SANDBOX_WRAPPER_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = env::current_exe() {
        let sibling = exe.parent().map(|dir| dir.join(SANDBOX_WRAPPER_NAME));
        if sibling.as_ref().is_some_and(|path| path.is_file()) {
            return sibling;
        }
    }
    find_in_path(SANDBOX_WRAPPER_NAME)
}

/// Layer 3 launch decision: executable plus the wrapper's real-browser override.
#[cfg(feature = "chromium")]
pub(crate) struct SandboxLaunch {
    pub(crate) executable: Option<PathBuf>,
    pub(crate) real_binary: Option<PathBuf>,
}

/// Pick the Layer 3 launch executable: the wrapper when the sandbox is
/// usable, else the gate-certified binary / chromiumoxide auto-detection.
/// Every decision is a structured event; a missing prerequisite is never
/// fatal to the fetch.
#[cfg(feature = "chromium")]
pub(crate) fn sandbox_wrapper_or_binary(configured: Option<&Path>) -> SandboxLaunch {
    let sandbox = SandboxConfig::resolve();
    let fallback = || SandboxLaunch {
        executable: configured.map(Path::to_path_buf),
        real_binary: None,
    };
    // The wrapper execs the real browser, so it needs the path. When the
    // caller pinned none, resolve it with chromiumoxide's own detection
    // (`CHROME`, usual install paths, registry) instead of the wrapper's
    // short PATH list — otherwise routing the launch through the wrapper
    // would swap (or fail to find) the browser chromiumoxide would have
    // picked on its own.
    let real_binary = configured.map(Path::to_path_buf).or_else(|| {
        chromiumoxide::detection::default_executable(
            chromiumoxide::detection::DetectionOptions::default(),
        )
        .ok()
    });
    if let Some(reason) = sandbox.skip_reason() {
        tracing::warn!(
            event = "sandbox_skipped",
            layer = 3,
            reason,
            task_id = %sandbox.task_id,
            "sandbox unavailable — launching chrome unsandboxed"
        );
        return fallback();
    }
    match resolve_wrapper_path() {
        Some(path) => {
            tracing::info!(
                event = "sandbox_applied",
                layer = 3,
                sandbox = "arapuca-wrapper",
                task_id = %sandbox.task_id,
                wrapper = %path.display(),
                "browser launch sandboxed"
            );
            SandboxLaunch {
                executable: Some(path),
                real_binary,
            }
        },
        None => {
            tracing::warn!(
                event = "sandbox_skipped",
                layer = 3,
                reason = "sandbox wrapper binary not found",
                task_id = %sandbox.task_id,
                "sandbox unavailable — launching chrome unsandboxed"
            );
            fallback()
        },
    }
}

#[cfg(all(test, unix))]
#[path = "downloader_factory/tests.rs"]
mod tests;
