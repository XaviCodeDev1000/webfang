use super::js_strategy::*;
use super::*;
use crate::domain::JsStrategy;

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
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
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

// ========================================================================
// #2722 Phase 2 — obscura build-variant diagnostic (render capability)
// ========================================================================

/// Write an executable fake obscura that answers `--version` with a valid
/// semver and, when invoked as the variant probe (`fetch … --screenshot
/// <file>`), behaves per `render_build`. The probe only reads the exit
/// status, so the render arm just touches the target file.
#[cfg(unix)]
fn write_variant_probe_obscura(dir: &std::path::Path, render_build: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_path = dir.join("obscura");
    let render_arm = if render_build {
        "  touch \"$6\"\n  exit 0\n"
    } else {
        // Upstream's no-render refusal: clear message on stderr, non-zero exit.
        "  echo \"--screenshot requires a build with the render feature\" >&2\n  exit 1\n"
    };
    // Probe argv: $1=fetch $2=--dump $3=html $4=about:blank $5=--screenshot $6=<file>.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo \"obscura 0.2.7\"\n  exit 0\nfi\nif [ \"$1\" = \"fetch\" ]; then\n{render_arm}fi\nexit 0\n",
        render_arm = render_arm,
    );
    std::fs::write(&bin_path, script).expect("write fake obscura");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake obscura");
    bin_path
}

/// S2 (#2722 Phase 2): a `no-render` build refuses `--screenshot` with a
/// non-zero exit; the probe must classify that build as `Missing`.
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn render_probe_classifies_no_render_build() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = write_variant_probe_obscura(tmp.path(), false);
    assert_eq!(
        probe_render_capability(&bin),
        RenderCapability::Missing,
        "a build refusing --screenshot is a no-render build"
    );
}

/// S2: a render-capable build paints `about:blank` and exits 0 → `Capable`.
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn render_probe_classifies_render_build() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = write_variant_probe_obscura(tmp.path(), true);
    assert_eq!(
        probe_render_capability(&bin),
        RenderCapability::Capable,
        "a build accepting --screenshot is render-capable"
    );
}

/// Pure classifier contract (Miri-safe): no output → `Unknown` (no signal
/// must not invent a diagnostic), success → `Capable`, failure → `Missing`.
#[test]
fn classify_render_probe_maps_output_shapes() {
    // `ExitStatus::from_raw` lives on a platform-specific extension trait.
    // Import the matching one so this pure, Miri-safe test also type-checks
    // under the Windows cross-lint target (invisible to the Linux gate).
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    let ok = std::process::Output {
        status: std::process::ExitStatus::from_raw(0),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let failed = std::process::Output {
        status: std::process::ExitStatus::from_raw(1),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    assert_eq!(classify_render_probe(None), RenderCapability::Unknown);
    assert_eq!(classify_render_probe(Some(&ok)), RenderCapability::Capable);
    assert_eq!(
        classify_render_probe(Some(&failed)),
        RenderCapability::Missing
    );
}

/// S2: the variant diagnostic is a WARNING, never a gate failure — a
/// `no-render` build must pass the Hybrid preflight (verified upstream: the
/// `--dump html` path is feature-independent, only paint resources are
/// gated by the render feature).
#[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
#[cfg(unix)]
#[test]
fn no_render_build_passes_preflight_with_warning() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = write_variant_probe_obscura(tmp.path(), false);
    let mut opts = CrawlOptions::default();
    opts.network.js_strategy = JsStrategy::Hybrid;
    opts.network.obscura_binary = bin.to_string_lossy().into_owned();
    assert!(
        check_js_dependencies_with(
            &["definitely-not-installed-9x4k"],
            true,
            &tmp.path().to_string_lossy(),
            &opts
        )
        .is_ok(),
        "a no-render build is a diagnostic warning, not a gate failure"
    );
}
