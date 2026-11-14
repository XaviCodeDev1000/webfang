use super::js_strategy::*;

// ---- XP-S-06 (#1608) — bounded --version probes ----------------------

/// A probe whose binary exits normally reports status + captured output.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn version_probe_reports_normal_exit_and_output() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("probe");
    std::fs::write(&bin_path, "#!/bin/sh\necho 'obscura 0.2.0'\n").expect("write probe");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x probe");

    let output = run_version_probe_with_timeout(&bin_path, std::time::Duration::from_secs(5))
        .expect("probe must complete");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("0.2.0"));
}

/// A probe that never exits is killed at the deadline and reports `None`
/// — a wedged `--version` can no longer hang preflight forever.
#[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn version_probe_kills_a_wedged_binary_at_the_deadline() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("probe");
    std::fs::write(&bin_path, "#!/bin/sh\nsleep 30\n").expect("write probe");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x probe");

    let started = std::time::Instant::now();
    let outcome = run_version_probe_with_timeout(&bin_path, std::time::Duration::from_millis(150));
    assert!(outcome.is_none(), "a wedged probe must report None");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the probe must return promptly after killing the child"
    );
}
