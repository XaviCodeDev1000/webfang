#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2784: the connection-failure taxonomy.
//!
//! The trace's error events must carry a **machine-readable cause**. Today they
//! carry only a message string, and six measured failure causes collapse into
//! three or four of those strings — so an operator (and any consumer of the
//! JSONL) cannot tell a refused connection from a dropped SYN, nor a DNS
//! failure from either.
//!
//! The contract is fixed in `odd/tasks/connection-cause-taxonomy.md`: the field
//! is `cause`, and its values are stable (`dns_resolution`, `connect_refused`,
//! `connect_timeout`, `read_timeout`, `connection_closed`, `tls_handshake`,
//! `unknown`). `unknown` is a value, not a shrug: it means the classifier ran
//! and could not decide.
//!
//! This test needs no new API to be written: it runs the binary against a
//! CLOSED port (nothing listens on port 9 — the same proof the SSRF guard's
//! own tests use) and asserts the trace classifies the failure.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::cmd;
use webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV;

/// The `cause` values the trace may carry.
const CAUSE_FIELD: &str = "cause";

/// Every value stored under `field` in the `fields` object of each JSONL
/// event, in observation order.
///
/// Both channels are the same walk with a different key: the machine contract
/// (`cause`) and the rendered message (`error`).
fn values_in(trace: &std::path::Path, field: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(trace) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| {
            event
                .get("fields")
                .and_then(|fields| fields.get(field))
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
        })
        .collect()
}

/// Arrange: a batch run that writes its trace to `trace`.
///
/// Both hatches are the documented test-only ones, and they are armed on the
/// spawned child's environment only — the same posture the pacing tests use.
fn batch_with_trace(output: &std::path::Path, trace: &std::path::Path) -> assert_cmd::Command {
    let mut batch_cmd = cmd();
    batch_cmd
        .env(DISABLE_VALIDATING_RESOLVER_ENV, "1")
        // The loopback fixture is refused by the production SSRF entry
        // guard, so the test arms ONLY the documented test-only hatch.
        .env("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1")
        .arg("--batch")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(output)
        .arg("--trace-file")
        .arg(trace);
    batch_cmd
}

/// Arrange: a `--url` scrape that writes its trace to `trace`.
///
/// The same two documented test-only hatches as the batch route, armed on the
/// spawned child's environment only.
fn url_with_trace(
    url: &str,
    output: &std::path::Path,
    trace: &std::path::Path,
) -> assert_cmd::Command {
    let mut url_cmd = cmd();
    url_cmd
        .env(DISABLE_VALIDATING_RESOLVER_ENV, "1")
        .env("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1")
        .arg("--url")
        .arg(url)
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output)
        .arg("--trace-file")
        .arg(trace);
    url_cmd
}

/// One observed run: what the trace said, in both channels.
struct Observed {
    /// The `cause` of every event that carried one.
    causes: Vec<String>,
    /// Every rendered error message, joined by newlines.
    errors: String,
}

impl Observed {
    fn from_trace(trace: &std::path::Path) -> Self {
        Self {
            causes: values_in(trace, CAUSE_FIELD),
            errors: values_in(trace, "error").join("\n"),
        }
    }
}

/// Run one command to failure in a fresh directory and read its trace back.
///
/// Every test here needs the same four things: an isolated output directory, a
/// trace file, a proof that the run actually failed, and both channels of the
/// trace. The route decides only how the command is built.
fn observe(
    configure: impl FnOnce(&std::path::Path, &std::path::Path) -> assert_cmd::Command,
) -> Observed {
    let out = tempfile::TempDir::new().expect("temp output dir");
    let trace = out.path().join("trace.jsonl");
    configure(out.path(), &trace)
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .failure();
    Observed::from_trace(&trace)
}

/// Run a batch scrape against `url`.
///
/// The target is unreachable by construction, so the run is expected to fail:
/// a success would mean the taxonomy was never exercised.
fn run_batch(url: &str) -> Observed {
    observe(|output, trace| {
        let mut command = batch_with_trace(output, trace);
        command.write_stdin(url.to_string());
        command
    })
}

/// The `--url` route of [`run_batch`].
fn run_url(url: &str) -> Observed {
    observe(|output, trace| url_with_trace(url, output, trace))
}

/// Positive: a refused connection is classified in the trace (#2784).
///
/// Port 9 (discard) has nothing listening on it, so the connection is refused
/// by the kernel — the cheapest reproducible transport failure, and the one
/// the taxonomy must name.
#[tokio::test]
async fn a_refused_connection_is_classified_in_the_trace() {
    let causes = run_batch("http://127.0.0.1:9/").causes;
    assert!(
        causes.iter().any(|cause| cause == "connect_refused"),
        "the trace must classify the refused connection as `connect_refused`; causes found: {causes:?}"
    );
}

/// Positive: a name that does not resolve is classified as `dns_resolution` (#2784).
///
/// The `.invalid` TLD is reserved (RFC 2606), so the name cannot exist: the
/// resolver answers, the lookup fails, and the failure is a RESOLUTION
/// failure — the class that today shares its message with a refused
/// connection (`client error (Connect)` at 0 s).
#[tokio::test]
async fn an_unresolvable_name_is_classified_in_the_trace() {
    let causes = run_batch("http://webfang-lab-nonexistent.invalid/").causes;
    assert!(
        causes.iter().any(|cause| cause == "dns_resolution"),
        "the trace must classify the unresolvable name as `dns_resolution`; causes found: {causes:?}"
    );
}

/// #2784: the `--url` route classifies the failure too.
///
/// The cause used to be computed and then DISCARDED here: the narrowing of
/// the transport error produced `Io`, `Dns`, `Tls` or `Timeout` for most real
/// failures, and only the `Network` variant carried the carrier. A refused
/// connection — the most common failure of all — reached the trace with NO
/// cause at all.
#[tokio::test]
async fn the_url_route_classifies_a_refused_connection() {
    let causes = run_url("http://127.0.0.1:9/").causes;
    assert!(
        causes.iter().any(|cause| cause == "connect_refused"),
        "the `--url` route must classify the refused connection as `connect_refused`; causes found: {causes:?}"
    );
}

/// #2784: the `--url` route classifies a name that does not resolve.
///
/// The resolver's own typed failure used to be narrowed away by the text
/// match on "dns error", so the DNS class could not be reached on this route.
#[tokio::test]
async fn the_url_route_classifies_an_unresolvable_name() {
    let causes = run_url("http://webfang-lab-nonexistent.invalid/").causes;
    assert!(
        causes.iter().any(|cause| cause == "dns_resolution"),
        "the `--url` route must classify the unresolvable name as `dns_resolution`; causes found: {causes:?}"
    );
}

/// #2784: the user-facing message names the cause.
///
/// The issue's first acceptance criterion: a resolution failure and a
/// refused connection produce DIFFERENT messages, each naming its cause. The
/// operator reads the message; the trace's `cause` field is the machine
/// contract that goes with it.
#[tokio::test]
async fn the_user_facing_message_names_the_cause() {
    let refused = run_batch("http://127.0.0.1:9/").errors;
    let dns = run_batch("http://webfang-lab-nonexistent.invalid/").errors;

    assert!(
        refused.contains("el servidor rechazó la conexión"),
        "the refused connection's message must name its cause; messages: {refused}"
    );
    assert!(
        dns.contains("no se pudo resolver el nombre del dominio"),
        "the resolution failure's message must name its cause; messages: {dns}"
    );
    assert_ne!(
        refused, dns,
        "two different causes must not produce the same message"
    );
}
