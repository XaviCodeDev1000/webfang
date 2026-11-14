//! JS strategy dependency preflight (#685): Chrome/Obscura binary gates.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::JsStrategy;
use crate::CliExit;

use super::path_resolution::{
    has_path_separator, resolve_executable_in_path, resolve_obscura_binary,
};

// ============================================================================
// JS strategy dependency preflight (#685)
// ============================================================================

/// Chrome binary candidates probed for `--js-strategy full` (#685).
///
/// The audit spec named `google-chrome` only, but Linux distributions ship
/// the engine under several names, so one missing distro binary must not
/// mask an installed Chrome. Order matters: the first binary that reports a
/// version wins.
///
/// Windows/macOS use their own candidate sets ([`default_chrome_candidates`],
/// XP-S-04 #1608), so this list is only compiled on the platforms it serves.
#[cfg(not(any(windows, target_os = "macos")))]
const DEFAULT_CHROME_CANDIDATES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium-browser",
    "chromium",
];

/// Chrome/Chromium candidates for the current platform (XP-S-04, #1608).
///
/// Order matters: the first candidate that resolves to an existing file
/// reporting a `--version` wins.
///
/// - Linux: bare names resolved through `PATH` (list unchanged from #685).
/// - Windows: the standard `chrome.exe` install locations first (Program
///   Files, Program Files (x86), then the per-user `%LOCALAPPDATA%`
///   install), then the bare `chrome` name through `PATH`/`PATHEXT`.
/// - macOS: the `.app` bundle executables, plus bare `chromium` for
///   Homebrew-style installs.
///
/// Injectability is preserved: [`resolve_chrome_binary_with`] and
/// [`check_js_dependencies_with`] still take the candidate list as a
/// parameter — only the production default is platform-conditional.
fn default_chrome_candidates() -> Vec<String> {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        DEFAULT_CHROME_CANDIDATES
            .iter()
            .map(|s| (*s).into())
            .collect()
    }
    #[cfg(windows)]
    {
        let mut candidates: Vec<String> = Vec::new();
        for base in [
            r"C:\Program Files\Google\Chrome\Application",
            r"C:\Program Files (x86)\Google\Chrome\Application",
        ] {
            candidates.push(format!(r"{base}\chrome.exe"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let per_user = std::path::PathBuf::from(local_app_data)
                .join(r"Google\Chrome\Application\chrome.exe");
            candidates.push(per_user.to_string_lossy().into_owned());
        }
        // Bare name last: Chrome rarely registers on PATH, but the
        // PATH/PATHEXT search is free to try.
        candidates.push("chrome".to_string());
        candidates
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "chromium".to_string(),
        ]
    }
}

/// Resolve the Chrome/Chromium binary the gate certified (F-52-c, #1278).
///
/// Returns the first candidate (in order) that resolves to an existing
/// *file* and whose `--version` probe exits successfully. Bare names are
/// resolved through the injected `PATH` exactly like the OS would
/// ([`resolve_executable_in_path`]); explicit paths must exist as files.
/// The `is_file` requirement (not `exists`) excludes the
/// `/opt/google/chrome` *directory* trap that chromiumoxide's own
/// `get_by_path` falls into via `exists()`.
///
/// Pure and injectable like the rest of the gate (#787): tests pass
/// controlled candidates + `PATH` instead of touching process-global env.
pub(super) fn resolve_chrome_binary_with(candidates: &[&str], path_value: &str) -> Option<PathBuf> {
    candidates.iter().find_map(|binary| {
        let resolved = if has_path_separator(binary) {
            let path = PathBuf::from(binary);
            path.is_file().then_some(path)
        } else {
            resolve_executable_in_path(binary, path_value)
        };
        resolved.filter(|path| binary_path_reports_version(path))
    })
}

/// Process-PATH entry point for `resolve_chrome_binary_with`: the
/// production resolution over the platform default candidates
/// the platform default candidates (`default_chrome_candidates`). Called once
/// after the gate passes
/// (`main.rs` 6c); the result travels in `CrawlOptions.network.chrome_binary`
/// so the launcher runs exactly the certified binary.
pub fn resolve_chrome_binary() -> Option<PathBuf> {
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    resolve_chrome_binary_with(&candidate_refs, &path_value)
}

/// Upper bound for one `--version` probe (XP-S-06, #1608).
///
/// The probe runs synchronously, pre-crawl; a wedged or interactive binary
/// must not hang the CLI forever. On expiry the child is killed and the
/// probe reports failure (the Full gate then goes red; the Hybrid Obscura
/// check degrades to "unknown version" with a warning).
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Poll interval while waiting for a `--version` probe to exit.
const VERSION_PROBE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Run `<path> --version` silently and bounded (XP-S-06, #1608).
///
/// Returns the process `Output` (status + captured stdout/stderr) or `None`
/// when the spawn fails, the wait errors, or the deadline elapses first
/// (the child is killed and reaped before returning). The probe probes the
/// exact file the launcher will execute, so gate-certified and launched are
/// the same binary by construction; output is captured because the Obscura
/// version check parses it.
pub(super) fn run_version_probe_with_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read as _;
                // The child has exited: the pipes are closed on its side, so
                // read_to_end drains the buffered output without blocking.
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_end(&mut stderr);
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            },
            Ok(None) => {},
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(VERSION_PROBE_POLL);
    }
}

/// Silent `--version` probe against an already-resolved file path, bounded
/// by [`VERSION_PROBE_TIMEOUT`].
///
/// Output is discarded — only the exit status matters.
fn binary_path_reports_version(path: &Path) -> bool {
    matches!(
        run_version_probe_with_timeout(path, VERSION_PROBE_TIMEOUT),
        Some(output) if output.status.success()
    )
}

/// Preflight: verify the local environment can satisfy the configured JS
/// strategy before any crawl starts (#685, #758, #787).
///
/// Strategy-specific checks:
///
/// - [`Static`](crate::domain::JsStrategy::Static): no external binary is
///   needed — returns immediately without touching disk or spawning
///   processes.
/// - [`Hybrid`](crate::domain::JsStrategy::Hybrid) (#787, #793): Layer 2 shells
///   out to Obscura (`ObscuraDownloader`), so the configured
///   `--obscura-binary` (default `obscura`) must exist. A value with a path
///   separator must exist as a file; a bare name must resolve via `PATH`.
///   This turns a missing binary into a clean config error (exit 78)
///   instead of a silent Layer-2 fallback mid-crawl. Once resolved, the
///   binary must report a version of at least `MINIMUM_OBSCURA_VERSION`
///   (#793): an older dump format would silently feed the wrong content
///   shape to Layer 2. A failing or unreadable `--version` probe degrades
///   to a warning (best-effort for unknown builds).
/// - [`Full`](crate::domain::JsStrategy::Full) renders pages through
///   Chromiumoxide, which spawns a real Chrome/Chromium binary. Two
///   independent preconditions are checked, in order:
///
///   1. **Compile-time capability** (#758): the binary must have been built
///      with the `chromium` feature. Without it, `ChromiumoxideDownloader`
///      is a stub that fails mid-crawl with an instruction the CLI binary
///      cannot follow. This is a build-configuration error, checked first
///      because probing the PATH is pointless when the binary cannot render
///      JS at all.
///   2. **Runtime environment**: probing the installed candidates turns a
///      missing browser into a clean config error (exit 78) instead of a
///      confusing mid-crawl browser launch failure.
///
/// Runs once, in a synchronous context, before crawl start — a brief
/// blocking `--version` probe (Full and Hybrid) is acceptable there.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the strategy is
/// [`Hybrid`](crate::domain::JsStrategy::Hybrid) and the configured Obscura
/// binary does not exist (neither as a path nor on `PATH`) or reports a
/// version older than `MINIMUM_OBSCURA_VERSION` (#793), or when it is
/// [`Full`](crate::domain::JsStrategy::Full) and either the binary was
/// built without the `chromium` feature, or no Chrome/Chromium candidate
/// on `PATH` reports a version.
pub fn check_js_dependencies(opts: &CrawlOptions) -> Result<(), CliExit> {
    // The PATH value is injected so the core check stays pure and testable —
    // tests pass a controlled PATH instead of mutating process-global env.
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    check_js_dependencies_with(
        &candidate_refs,
        cfg!(feature = "chromium"),
        &path_value,
        opts,
    )
}

/// Candidate-, feature-, and PATH-injectable core of
/// [`check_js_dependencies`] — Full/Chrome tests use fake executables in a
/// controlled PATH dir (F-52-c), Hybrid tests do the same for obscura.
/// inject the feature flag because `cfg!` cannot be toggled per test, and
/// inject the `PATH` value so the Hybrid Obscura lookup never races with
/// concurrent tests over the process-global environment (#787).
pub(super) fn check_js_dependencies_with(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    match opts.network.js_strategy {
        // Static crawls with wreq only — no external binary to check.
        JsStrategy::Static => Ok(()),
        // Hybrid escalates through Obscura: fail fast when the binary is
        // missing instead of letting Layer 2 fail in the middle of the crawl
        // (#787).
        JsStrategy::Hybrid => check_obscura_binary(&opts.network.obscura_binary, path_value),
        JsStrategy::Full => check_chrome_binary(candidates, chromium_enabled, path_value),
    }
}

/// Full-strategy check (#685, #758, F-52-c #1278): `chromium` feature + a
/// Chrome/Chromium candidate that reports a version. The gate now resolves
/// through [`resolve_chrome_binary_with`] — the same resolution the launcher
/// will use — so a green gate means the certified binary is launchable.
fn check_chrome_binary(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
) -> Result<(), CliExit> {
    if !chromium_enabled {
        return Err(CliExit::ConfigError(
            "--js-strategy full requiere un binario compilado con la feature `chromium`; \
                     recompilá con --features chromium o usá --js-strategy hybrid"
                .into(),
        ));
    }

    let chrome_present = resolve_chrome_binary_with(candidates, path_value).is_some();
    if chrome_present {
        tracing::info!(strategy = "full", "chrome_dependency_checked");
        return Ok(());
    }

    Err(CliExit::ConfigError(
        "--js-strategy full requiere Google Chrome instalado".into(),
    ))
}

/// Hybrid-strategy check (#787): the configured Obscura binary exists.
fn check_obscura_binary(binary: &str, path_value: &str) -> Result<(), CliExit> {
    match resolve_obscura_binary(binary, path_value) {
        Some(resolved) => check_obscura_version(binary, &resolved),
        None if has_path_separator(binary) => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario obscura: la ruta \"{binary}\" no existe \
             o no es un archivo; verificá --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
        None => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario \"{binary}\": no se encontró en PATH; \
             instalalo o configurá una ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}

/// Minimum Obscura version for the Layer 2 contract (#793, #1710).
///
/// Two floors, one constant. The dump-format contract: `obscura fetch --dump
/// html` was verified in 0.2.0 and did not change through 0.2.3, so an older
/// binary may change dump semantics and silently feed the wrong content shape
/// to Layer 2. And the upstream security fixes a scraper that fetches
/// adversarial URLs cannot sit below: 0.2.2 closed SSRF gaps (IPv6 embedded /
/// 6to4 / NAT64, CGNAT, IANA ranges) and 0.2.3 hardened cookies (host-only,
/// public-suffix, SameSite), stripped credentials from cross-origin redirects
/// and updated rustls (RUSTSEC-2026-0285).
///
/// The floor is 0.2.5, the current stable upstream release.
const MINIMUM_OBSCURA_VERSION: (u64, u64, u64) = (0, 2, 5);

/// User-facing spelling of [`MINIMUM_OBSCURA_VERSION`] for Spanish errors.
pub(super) const MINIMUM_OBSCURA_VERSION_STR: &str = "0.2.5";

/// Verdict of the Obscura version assessment (#793).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VersionVerdict {
    /// Parsed version is >= the minimum.
    Meets,
    /// Parsed version is below the minimum.
    TooOld,
    /// Probe failed or output was unparseable — degrade, do not block.
    Unknown,
}

/// Parse a pure MAJOR.MINOR.PATCH token into a semantic triple.
///
/// Tolerates a leading `v`/`V`, a `-`/`+` suffix on the patch segment
/// (`0.2.0-rc.1`, `0.2.0+build`), and extra dotted segments (ignored).
fn parse_version_token(token: &str) -> Option<(u64, u64, u64)> {
    let cleaned = token.strip_prefix(['v', 'V']).unwrap_or(token);
    let mut parts = cleaned.splitn(4, '.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?.split(['-', '+']).next()?;
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

/// Extract the first semver-like version from raw `--version` output (#793).
///
/// Pure: no process or environment access — unit-tested without mutation.
pub(super) fn parse_obscura_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(parse_version_token)
}

/// Classify an optional parsed version against the minimum (#793). Pure.
pub(super) fn assess_obscura_version(parsed: Option<(u64, u64, u64)>) -> VersionVerdict {
    match parsed {
        Some(version) if version >= MINIMUM_OBSCURA_VERSION => VersionVerdict::Meets,
        Some(_) => VersionVerdict::TooOld,
        None => VersionVerdict::Unknown,
    }
}

/// Run `<resolved> --version` once and parse its output (#793), bounded by
/// [`VERSION_PROBE_TIMEOUT`] (XP-S-06, #1608).
///
/// Returns `None` when the probe cannot run, is killed at the deadline,
/// exits non-zero, or prints no semver-like token on stdout or stderr.
/// Blocking spawn: acceptable once, pre-crawl (same silent-probe shape the
/// Full gate uses per resolved file).
fn probe_obscura_version(resolved: &std::path::Path) -> Option<(u64, u64, u64)> {
    let output = run_version_probe_with_timeout(resolved, VERSION_PROBE_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_obscura_version(&stdout)
        .or_else(|| parse_obscura_version(&String::from_utf8_lossy(&output.stderr)))
}

/// Spanish operator-facing text of the `Unknown` Obscura version verdict
/// (#1710).
///
/// A verdict that says only "the version could not be determined" is one the
/// operator cannot act on: the floor the guard enforces
/// ([`MINIMUM_OBSCURA_VERSION_STR`]) is the whole point of the check, so the
/// warning has to NAME it. The text is derived from the constant, so the
/// message can never drift from the floor the guard actually applies.
///
/// The verdict itself does not change: an unreadable build stays a
/// DEGRADATION — the crawl and the release flow do not stop. Only the
/// CONTENT of the warning changes.
fn unknown_obscura_version_warning() -> String {
    format!(
        "no se pudo determinar la versión de obscura; se esperaba al menos \
         {MINIMUM_OBSCURA_VERSION_STR} (la sonda --version falló o su salida no es \
         interpretable) — se continúa en modo best-effort"
    )
}

/// Version half of the Hybrid check (#793): enforce the minimum contract on
/// a resolved binary. An unreadable build degrades to a warning (unknown
/// custom builds must not be hard-blocked); a parseable older version fails
/// fast with exit 78.
fn check_obscura_version(binary: &str, resolved: &std::path::Path) -> Result<(), CliExit> {
    let parsed = probe_obscura_version(resolved);
    let version = parsed.map_or_else(
        || "unknown".to_string(),
        |(major, minor, patch)| format!("{major}.{minor}.{patch}"),
    );

    match assess_obscura_version(parsed) {
        VersionVerdict::Meets => {
            tracing::info!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                version = %version,
                "obscura_dependency_checked"
            );
            Ok(())
        },
        VersionVerdict::Unknown => {
            warn!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                expected_version = MINIMUM_OBSCURA_VERSION_STR,
                user_message = %unknown_obscura_version_warning(),
                "obscura_version_unreadable: --version probe failed or unparseable — \
                 continuing best-effort"
            );
            Ok(())
        },
        VersionVerdict::TooOld => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere obscura {MINIMUM_OBSCURA_VERSION_STR} o superior: \
             el binario \"{binary}\" reporta la versión {version}; actualizalo o cambiá la \
             ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}
