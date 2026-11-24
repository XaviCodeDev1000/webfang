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
const ARAPUCA_BINARY_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
/// Explicit run identifier forwarded to `arapuca --task-id`.
const RUN_ID_ENV: &str = "WEBFANG_RUN_ID";
/// Explicit tenant identifier, used when no run id is set.
const TENANT_ID_ENV: &str = "WEBFANG_TENANT_ID";
/// `on`/`off` override for the cgroup-delegation probe (`auto` otherwise).
const CGROUP_DELEGATION_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
/// Explicit override for the sandbox wrapper binary.
#[cfg(feature = "chromium")]
const SANDBOX_WRAPPER_ENV: &str = "WEBFANG_SANDBOX_WRAPPER";
/// Name of the sibling/PATH wrapper binary built by this workspace.
#[cfg(feature = "chromium")]
const SANDBOX_WRAPPER_NAME: &str = "webfang-sandbox-wrapper";

/// Mounts appended after the binary and profile directories. `/sys` itself
/// is never mounted — Chrome only needs these subtrees.
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
            task_id: sandbox_task_id(),
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

fn resolve_arapuca() -> Option<PathBuf> {
    if let Some(path) = env::var_os(ARAPUCA_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    find_in_path("arapuca")
}

fn sandbox_task_id() -> String {
    for key in [RUN_ID_ENV, TENANT_ID_ENV] {
        if let Some(value) = env::var(key).ok().filter(|v| !v.trim().is_empty()) {
            return value;
        }
    }
    format!("webfang-{}", std::process::id())
}

fn cgroup_delegation() -> bool {
    match env::var(CGROUP_DELEGATION_ENV).as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => cgroup_delegated(),
    }
}

fn cgroup_delegated() -> bool {
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

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

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

/// `arapuca run` argv in the exact spec order. Obscura has no resolved
/// profile directory, so the optional `-v <profile-dir>:rw` is omitted here.
pub(crate) fn arapuca_args(binary: &Path, task_id: &str, inner: &[String]) -> Vec<String> {
    let mut argv = vec![
        "run".to_string(),
        "--task-id".to_string(),
        task_id.to_string(),
        "--seccomp".to_string(),
        "baseline".to_string(),
    ];
    push_mount(&mut argv, &binary_directory(binary), "ro");
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

/// Pick the Layer 3 launch executable: the wrapper when the sandbox is
/// usable, else the gate-certified binary / chromiumoxide auto-detection.
/// Every decision is a structured event; a missing prerequisite is never
/// fatal to the fetch.
#[cfg(feature = "chromium")]
pub(crate) fn sandbox_wrapper_or_binary(configured: Option<&Path>) -> Option<PathBuf> {
    let sandbox = SandboxConfig::resolve();
    if let Some(reason) = sandbox.skip_reason() {
        tracing::warn!(
            layer = 3,
            reason,
            task_id = %sandbox.task_id,
            "sandbox unavailable — launching chrome unsandboxed"
        );
        return configured.map(Path::to_path_buf);
    }
    match resolve_wrapper_path() {
        Some(path) => {
            tracing::info!(
                layer = 3,
                sandbox = "arapuca-wrapper",
                task_id = %sandbox.task_id,
                wrapper = %path.display(),
                "browser launch sandboxed"
            );
            Some(path)
        },
        None => {
            tracing::warn!(
                layer = 3,
                reason = "sandbox wrapper binary not found",
                task_id = %sandbox.task_id,
                "sandbox unavailable — launching chrome unsandboxed"
            );
            configured.map(Path::to_path_buf)
        },
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use url::Url;

    use super::{arapuca_args, SandboxConfig};
    #[cfg(feature = "chromium")]
    use super::{resolve_wrapper_path, SANDBOX_WRAPPER_ENV};
    use crate::infrastructure::downloader::obscura_downloader::{Downloader, ObscuraDownloader};

    const ARAPUCA_ENV: &str = "WEBFANG_ARAPUCA_BINARY";
    const CGROUP_ENV: &str = "WEBFANG_CGROUP_DELEGATION";
    const TASK_ENV: &str = "WEBFANG_RUN_ID";
    const OBSCURA_BODY: &str =
        "printf '<p>ok</p>'\nprintf 'Page loaded: http://example.test/ - \"T\"\\n' >&2";
    /// Records its own argv next to the script, then execs the command after
    /// `--` so the existing stdout/stderr/exit-status contract is preserved.
    const ARAPUCA_BODY: &str = "for a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$(dirname \"$0\")/argv.txt\"; done\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"";

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fixture script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fixture script");
        path
    }

    async fn fetch_with(downloader: &ObscuraDownloader) -> String {
        let url = Url::parse("http://example.test/").expect("valid test URL");
        downloader
            .fetch(&url)
            .await
            .expect("fake obscura must succeed")
            .html
    }

    #[test]
    fn skip_reason_names_each_missing_prerequisite() {
        let missing_arapuca = SandboxConfig {
            arapuca: None,
            task_id: "webfang-1".into(),
            cgroup_delegated: true,
        };
        assert_eq!(
            missing_arapuca.skip_reason(),
            Some("arapuca binary not found")
        );

        let missing_cgroup = SandboxConfig {
            arapuca: Some(PathBuf::from("/usr/bin/arapuca")),
            task_id: "webfang-1".into(),
            cgroup_delegated: false,
        };
        assert_eq!(
            missing_cgroup.skip_reason(),
            Some("cgroup controllers not delegated")
        );

        let available = SandboxConfig {
            arapuca: Some(PathBuf::from("/usr/bin/arapuca")),
            task_id: "webfang-1".into(),
            cgroup_delegated: true,
        };
        assert_eq!(available.skip_reason(), None);
    }

    #[test]
    fn arapuca_args_follow_the_exact_spec_order() {
        let argv = arapuca_args(
            Path::new("/usr/bin/obscura"),
            "tenant-9",
            &["fetch".into(), "--dump".into(), "html".into()],
        );
        assert_eq!(
            argv,
            vec![
                "run",
                "--task-id",
                "tenant-9",
                "--seccomp",
                "baseline",
                "-v",
                "/usr/bin:ro",
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
                "/usr/bin/obscura",
                "fetch",
                "--dump",
                "html",
            ]
        );
    }

    #[tokio::test]
    async fn sandbox_passes_exact_arapuca_argv_and_preserves_the_fetch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let obscura = script(tmp.path(), "obscura", OBSCURA_BODY);
        let arapuca = script(tmp.path(), "arapuca", ARAPUCA_BODY);
        let _env = webfang_test_utils::EnvGuard::with(&[
            (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
            (CGROUP_ENV, "on"),
            (TASK_ENV, "tenant-7"),
        ]);

        let downloader = ObscuraDownloader::new(5, obscura.as_path());
        assert_eq!(fetch_with(&downloader).await, "<p>ok</p>");

        let recorded = std::fs::read_to_string(tmp.path().join("argv.txt")).expect("arapuca argv");
        let argv: Vec<&str> = recorded.lines().collect();
        let binary_dir = obscura
            .parent()
            .expect("fixture parent")
            .to_str()
            .expect("utf8 dir");
        assert_eq!(
            argv,
            vec![
                "run",
                "--task-id",
                "tenant-7",
                "--seccomp",
                "baseline",
                "-v",
                &format!("{binary_dir}:ro"),
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
                obscura.to_str().expect("utf8 path"),
                "fetch",
                "--dump",
                "html",
                "http://example.test/",
            ],
            "arapuca mount order must match the spec exactly"
        );
    }

    #[tokio::test]
    async fn missing_cgroup_delegation_skips_the_wrapper() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let obscura = script(tmp.path(), "obscura", OBSCURA_BODY);
        let arapuca = script(tmp.path(), "arapuca", ARAPUCA_BODY);
        let _env = webfang_test_utils::EnvGuard::with(&[
            (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
            (CGROUP_ENV, "off"),
            (TASK_ENV, "tenant-7"),
        ]);

        let downloader = ObscuraDownloader::new(5, obscura.as_path());
        assert_eq!(fetch_with(&downloader).await, "<p>ok</p>");
        assert!(
            !tmp.path().join("argv.txt").exists(),
            "no arapuca argv may be recorded when cgroups are not delegated"
        );
    }

    #[cfg(feature = "chromium")]
    #[test]
    fn wrapper_path_resolves_explicit_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wrapper = tmp.path().join("webfang-sandbox-wrapper");
        std::fs::write(&wrapper, "#!/bin/sh\nexit 0\n").expect("write fake wrapper");
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            SANDBOX_WRAPPER_ENV,
            wrapper.to_str().expect("utf8 path"),
        )]);
        assert_eq!(resolve_wrapper_path(), Some(wrapper));
    }
}
