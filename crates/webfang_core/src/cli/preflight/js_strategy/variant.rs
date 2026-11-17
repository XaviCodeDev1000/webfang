//! Obscura build-variant diagnostic (#2722 Phase 2).
//!
//! Upstream ships four release variants (default, `no-render`, `stealth`,
//! `no-render-stealth`). The `render` feature gates only paint resources
//! upstream (screenshot warmup, fonts, animation timeline): the `--dump
//! html` path Layer 2 consumes (DOM serialization, JS settle) is
//! feature-independent — verified against the v0.2.7 source. The variant
//! therefore cannot fail the gate: it is a diagnostic WARNING, so an
//! operator installing a `no-render` asset learns why `--screenshot`/PDF
//! would not work if they ever ask for them.

use std::path::Path;
use std::process::Command;

use tracing::warn;

/// Upper bound for one probe (XP-S-06, #1608): probes run synchronously,
/// pre-crawl; a wedged or interactive binary must not hang the CLI.
use super::VERSION_PROBE_TIMEOUT;

/// Poll interval while waiting for a probe to exit.
const PROBE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Verdict of the render-capability probe (#2722 Phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cli::preflight) enum RenderCapability {
    /// The build accepts `--screenshot`: the render feature is compiled in.
    Capable,
    /// The build refuses `--screenshot`: a `no-render` release variant.
    Missing,
    /// The probe could not run or timed out: no signal, stay silent.
    Unknown,
}

/// Run `<path> <args…>` silently and bounded (XP-S-06, #1608).
///
/// Returns the process `Output` (status + captured stdout/stderr) or `None`
/// when the spawn fails, the wait errors, or the deadline elapses first
/// (the child is killed and reaped before returning). Output is captured
/// because the callers classify it.
pub(super) fn run_probe_with_timeout(
    path: &Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    let mut child = Command::new(path)
        .args(args)
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
        std::thread::sleep(PROBE_POLL);
    }
}

/// Classify one render-probe output. Pure, Miri-safe.
pub(in crate::cli::preflight) fn classify_render_probe(
    output: Option<&std::process::Output>,
) -> RenderCapability {
    match output {
        None => RenderCapability::Unknown,
        Some(out) if out.status.success() => RenderCapability::Capable,
        Some(_) => RenderCapability::Missing,
    }
}

/// Probe whether the resolved obscura build has the `render` feature (#2722
/// Phase 2): ask it to screenshot `about:blank`. A render build paints and
/// exits 0; a `no-render` build refuses `--screenshot` with a clear message
/// and a non-zero exit. `about:blank` involves no network; the probe file
/// lands in the tempdir and is removed right after.
pub(in crate::cli::preflight) fn probe_render_capability(resolved: &Path) -> RenderCapability {
    let shot = std::env::temp_dir().join(format!(
        "webfang-obscura-variant-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or_default(),
    ));
    let _ = std::fs::remove_file(&shot);
    let shot_str = shot.to_string_lossy().into_owned();
    let output = run_probe_with_timeout(
        resolved,
        &[
            "fetch",
            "--dump",
            "html",
            "about:blank",
            "--screenshot",
            &shot_str,
        ],
        VERSION_PROBE_TIMEOUT,
    );
    let _ = std::fs::remove_file(&shot);
    classify_render_probe(output.as_ref())
}

/// Emit the render-variant diagnostic after the version gate passes (#2722
/// Phase 2). WARN only on a confirmed `no-render` build; an inconclusive
/// probe stays silent (no signal must not invent one).
pub(super) fn warn_on_missing_render_capability(binary: &str, resolved: &Path) {
    match probe_render_capability(resolved) {
        RenderCapability::Capable => tracing::info!(
            strategy = "hybrid",
            binary = %binary,
            variant = "render",
            "obscura_variant_checked"
        ),
        RenderCapability::Missing => warn!(
            strategy = "hybrid",
            binary = %binary,
            variant = "no-render",
            user_message = "el binario obscura no tiene la feature de render (variante \
                            no-render): --screenshot y PDF no están disponibles; el \
                            volcado HTML de Layer 2 no se ve afectado",
            "obscura_build_lacks_render_feature: diagnostic only — L2 dump semantics \
             unaffected"
        ),
        RenderCapability::Unknown => tracing::debug!(
            strategy = "hybrid",
            binary = %binary,
            "obscura_variant_probe_inconclusive: no diagnostic emitted"
        ),
    }
}
