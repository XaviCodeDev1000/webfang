use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use url::Url;

use super::{arapuca_args, resolve_executable, SandboxConfig};
#[cfg(feature = "chromium")]
use super::{resolve_wrapper_path, sandbox_wrapper_or_binary, SANDBOX_WRAPPER_ENV};
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
    // S3 rule 5: the wrapper is Linux-only, so non-Linux runners skip before
    // resolving prerequisites and report the platform reason instead.
    let expected_missing_arapuca = if cfg!(target_os = "linux") {
        Some("arapuca binary not found")
    } else {
        Some("non-linux platform")
    };
    assert_eq!(missing_arapuca.skip_reason(), expected_missing_arapuca);

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
        None,
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

// S3 rule 5 makes the wrapper Linux-only: the argv/spawn contracts below only
// exist where `SandboxConfig::skip_reason` can return None.
#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(miri, ignore = "posix_spawnattr_init unsupported by Miri")]
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

#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(miri, ignore = "posix_spawnattr_init unsupported by Miri")]
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

/// The default configuration names a bare binary (`obscura`); the sandbox
/// mount must be the directory that actually holds it.
#[cfg(target_os = "linux")]
#[tokio::test]
#[cfg_attr(miri, ignore = "posix_spawnattr_init unsupported by Miri")]
async fn bare_binary_name_mounts_the_directory_that_actually_holds_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let obscura = script(tmp.path(), "obscura", OBSCURA_BODY);
    let arapuca = script(tmp.path(), "arapuca", ARAPUCA_BODY);
    // PREPEND the fixture dir: the fake arapuca script itself calls
    // `dirname`, so replacing PATH wholesale would break the fixture.
    let path_var = format!(
        "{}:{}",
        tmp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = webfang_test_utils::EnvGuard::with(&[
        (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
        (CGROUP_ENV, "on"),
        (TASK_ENV, "tenant-8"),
        ("PATH", path_var.as_str()),
    ]);

    let downloader = ObscuraDownloader::new(5, "obscura");
    assert_eq!(fetch_with(&downloader).await, "<p>ok</p>");

    let recorded = std::fs::read_to_string(tmp.path().join("argv.txt")).expect("arapuca argv");
    let argv: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        argv[6],
        format!("{}:ro", tmp.path().display()),
        "the :ro mount must be the binary's real directory, never the cwd"
    );
    assert_eq!(
        argv[argv.len() - 5],
        obscura.to_str().expect("utf8 path"),
        "the sandboxed exec must use the resolved absolute binary"
    );
}

#[test]
fn resolve_executable_never_falls_back_to_the_current_directory() {
    assert_eq!(
        resolve_executable(Path::new("webfang-definitely-not-an-executable")),
        None,
        "an unresolved bare name must skip the sandbox, never mount the cwd"
    );
    assert_eq!(
        resolve_executable(Path::new("/usr/bin/obscura")),
        Some(PathBuf::from("/usr/bin/obscura"))
    );
    let relative = resolve_executable(Path::new("./bin/obscura"))
        .expect("a relative path with a separator resolves against the cwd");
    assert!(relative.is_absolute());
}

/// Chromiumoxide's detection honours `CHROME`; the wrapper must receive
/// that resolved path when the caller did not pin one.
#[cfg(feature = "chromium")]
#[test]
fn auto_detected_browser_is_forwarded_through_the_wrapper_override() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let arapuca = script(tmp.path(), "arapuca", "exit 0\n");
    let wrapper = script(tmp.path(), "webfang-sandbox-wrapper", "exit 0\n");
    let detected = script(tmp.path(), "branded-chrome", "exit 0\n");
    let _env = webfang_test_utils::EnvGuard::with(&[
        (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
        (CGROUP_ENV, "on"),
        (TASK_ENV, "tenant-7"),
        (SANDBOX_WRAPPER_ENV, wrapper.to_str().expect("utf8 path")),
        ("CHROME", detected.to_str().expect("utf8 path")),
    ]);

    let launch = sandbox_wrapper_or_binary(None);
    assert_eq!(launch.executable, Some(wrapper));
    assert_eq!(launch.real_binary, Some(detected));
}

#[cfg(feature = "chromium")]
#[test]
fn configured_binary_is_forwarded_through_the_wrapper_override() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let arapuca = script(tmp.path(), "arapuca", "exit 0\n");
    let wrapper = script(tmp.path(), "webfang-sandbox-wrapper", "exit 0\n");
    let pinned = PathBuf::from("/opt/google/chrome/chrome");
    let _env = webfang_test_utils::EnvGuard::with(&[
        (ARAPUCA_ENV, arapuca.to_str().expect("utf8 path")),
        (CGROUP_ENV, "on"),
        (TASK_ENV, "tenant-7"),
        (SANDBOX_WRAPPER_ENV, wrapper.to_str().expect("utf8 path")),
    ]);
    let launch = sandbox_wrapper_or_binary(Some(&pinned));
    assert_eq!(launch.executable, Some(wrapper));
    assert_eq!(launch.real_binary, Some(pinned));
}
