//! ci_plan — Rust port of `scripts/ci_plan.sh` (#2282).
//!
//! Resolves the change plan for every CI event. The event does not decide the
//! depth; the diff does. This binary resolves comparison SHAs for
//! pull_request/merge_group/push, asks the Bash classifiers for the verdict
//! (glue stays in Bash), and fails closed to FULL scope on any missing SHA,
//! detector failure or unknown event.
//!
//! Output contract: the 15 `key=value` pairs are written to `$GITHUB_OUTPUT`
//! when set, otherwise to stdout. `scripts/test_ci_gate_structure.sh` reads
//! the `FULL_PRODUCERS`/`NARROW_PRODUCERS` literals below from this file, so
//! they must stay on their own lines with that exact spelling.

use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

// Single source of truth: read literally by test_ci_gate_structure.sh.
const PLAN_LITERALS: &str = r#"
FULL_PRODUCERS="toolchain,repo-guards,artifact-guards,merge-metrics-guards,fmt,clippy,code-quality,test-core,test-full,test-release-provenance,obscura-hybrid,security,doc-quality,coverage"
NARROW_PRODUCERS="toolchain,repo-guards,artifact-guards,merge-metrics-guards,security,doc-quality"
"#;

const USAGE: &str = "usage: ci_plan\n\nReads EVENT_NAME plus the matching event SHA variables from the environment:\n  pull_request  PR_BASE_SHA, PR_HEAD_SHA\n  merge_group   MERGE_GROUP_BASE_SHA, MERGE_GROUP_HEAD_SHA\n  push          PUSH_BEFORE_SHA, PUSH_AFTER_SHA\nWrites the plan to $GITHUB_OUTPUT when set, otherwise stdout.\n";

/// Producer list from the literal block above (no duplicated constant).
fn literal(name: &str) -> &'static str {
    let prefix = format!("{name}=\"");
    for line in PLAN_LITERALS.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            return rest.trim_end_matches('"');
        }
    }
    // Unreachable: the literals are compiled into this file. Fails closed.
    ""
}

fn env_var(name: &str) -> String {
    env::var(name).unwrap_or_default()
}

/// A 40-char lowercase hex SHA that is not the all-zero sentinel.
fn valid_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !value.bytes().all(|b| b == b'0')
}

/// The event's comparison SHAs and its fail-closed reason when incomplete.
struct EventRefs {
    base: String,
    head: String,
    missing: &'static str,
}

fn event_refs(event: &str) -> EventRefs {
    match event {
        "pull_request" => EventRefs {
            base: env_var("PR_BASE_SHA"),
            head: env_var("PR_HEAD_SHA"),
            missing: "pull_request-missing-sha",
        },
        "merge_group" => EventRefs {
            base: env_var("MERGE_GROUP_BASE_SHA"),
            head: env_var("MERGE_GROUP_HEAD_SHA"),
            missing: "merge_group-missing-sha",
        },
        "push" => EventRefs {
            base: env_var("PUSH_BEFORE_SHA"),
            head: env_var("PUSH_AFTER_SHA"),
            missing: "push-missing-after",
        },
        _ => EventRefs {
            base: String::new(),
            head: String::new(),
            missing: "unknown-event",
        },
    }
}

/// Output sink: the append handle on $GITHUB_OUTPUT, or stdout.
enum Sink {
    File(File),
    Stdout(std::io::Stdout),
}

impl Sink {
    fn open() -> Result<Self, String> {
        let path = env_var("GITHUB_OUTPUT");
        if path.is_empty() {
            return Ok(Self::Stdout(std::io::stdout()));
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("cannot open GITHUB_OUTPUT {path}: {error}"))?;
        Ok(Self::File(file))
    }

    fn emit(&mut self, key: &str, value: &str) -> Result<(), String> {
        let line = format!("{key}={value}\n");
        match self {
            Self::File(file) => file
                .write_all(line.as_bytes())
                .map_err(|error| format!("write GITHUB_OUTPUT: {error}")),
            Self::Stdout(out) => out
                .write_all(line.as_bytes())
                .map_err(|error| format!("write stdout: {error}")),
        }
    }
}

/// Emit the complete FULL plan and explain the decision on stderr.
fn full_scope(
    sink: &mut Sink,
    event: &str,
    reason: &str,
    base: &str,
    head: &str,
) -> Result<(), String> {
    let base_out = if valid_sha(base) { base } else { "" };
    let head_out = if valid_sha(head) { head } else { "" };
    for (key, value) in [
        ("docs_only", "false"),
        ("ci_only", "false"),
        ("code_changed", "true"),
        ("all", "true"),
        ("affected", "all"),
        ("run_code_jobs", "true"),
        ("needs_ai", "true"),
        ("needs_mcp", "true"),
        ("needs_mutation_hotpath", "true"),
        ("snapshot_changed", "true"),
        ("needs_crossplatform", "true"),
        ("required_producers", literal("FULL_PRODUCERS")),
        (
            "scope_reason",
            &format!("event={event} mode=full reason={reason}"),
        ),
        ("base_sha", base_out),
        ("head_sha", head_out),
    ] {
        sink.emit(key, value)?;
    }
    eprintln!("ci-plan: event={event} -> full scope ({reason})");
    Ok(())
}

/// Prefix a detector's stderr lines the way `sed 's/^/  /'` does, preserving
/// whether the input ended in a newline (eprintln! would add one).
fn forward_detector_stderr(stderr: &[u8]) {
    let text = String::from_utf8_lossy(stderr);
    let mut segments = text.split('\n').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() && segment.is_empty() {
            break; // trailing newline: already emitted by the previous segment
        }
        eprint!("  {segment}");
        if segments.peek().is_some() {
            eprintln!();
        }
    }
}

/// Run one of the Bash detectors from the repository root, capturing both
/// streams. A non-zero status is reported as an error carrying its stderr, so
/// the caller can forward the detector's explanation once, without re-running it.
struct DetectorOutput {
    stdout: String,
}

fn detector(script: &Path, root: &Path, args: &[&str]) -> Result<DetectorOutput, Vec<u8>> {
    let output = Command::new("bash")
        .arg(script)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", script.display()).into_bytes())?;
    if !output.status.success() {
        return Err(output.stderr);
    }
    Ok(DetectorOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ci-plan: {error}");
            ExitCode::from(1)
        },
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("-h") | Some("--help") => {
            print!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        },
        Some(other) => {
            eprintln!("error: unknown argument '{other}' (see --help)");
            return Ok(ExitCode::from(2));
        },
        None => {},
    }

    let event = {
        let primary = env_var("EVENT_NAME");
        if primary.is_empty() {
            env_var("GITHUB_EVENT_NAME")
        } else {
            primary
        }
    };
    let event_label = if event.is_empty() { "unknown" } else { &event };
    let mut sink = Sink::open()?;

    let refs = event_refs(&event);
    if let Some(reason) = early_scope_reason(&event, &refs) {
        full_scope(&mut sink, event_label, reason, &refs.base, &refs.head)?;
        return Ok(ExitCode::SUCCESS);
    }

    let root = repo_root();
    let classifier = override_or_default("CI_PLAN_CLASSIFIER", &root, "ci_path_classifier.sh");
    let classifier_args = [
        "--base", &refs.base, "--head", &refs.head, "--format", "github",
    ];
    let classifier_out = match detector(&classifier, &root, &classifier_args) {
        Ok(output) => output.stdout,
        Err(stderr) => {
            eprintln!("ci-plan: classifier failed; failing closed");
            forward_detector_stderr(&stderr);
            full_scope(
                &mut sink,
                event_label,
                "classifier-failed",
                &refs.base,
                &refs.head,
            )?;
            return Ok(ExitCode::SUCCESS);
        },
    };

    let kv = parse_kv(&classifier_out);
    let run_code_jobs = kv.get("run_code_jobs").map(String::as_str).unwrap_or("");
    if run_code_jobs != "true" && run_code_jobs != "false" {
        full_scope(
            &mut sink,
            event_label,
            "classifier-missing-run_code_jobs",
            &refs.base,
            &refs.head,
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    let needs_crossplatform = run_code_jobs == "true" && crossplatform_needed(&root, &refs);

    emit_path_plan(
        &mut sink,
        &event,
        &refs,
        &kv,
        run_code_jobs,
        needs_crossplatform,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Ask the Bash crossplatform detector; any failure or non-tri-state answer
/// fails closed to `true`.
fn crossplatform_needed(root: &std::path::Path, refs: &EventRefs) -> bool {
    let detector_path =
        override_or_default("CI_PLAN_CROSSPLATFORM", root, "ci_crossplatform_scope.sh");
    let args = ["--base", &refs.base, "--head", &refs.head];
    match Command::new("bash")
        .arg(&detector_path)
        .args(args)
        .current_dir(root)
        .output()
    {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_string();
            match value.as_str() {
                "true" | "false" => value == "true",
                other => {
                    eprintln!("ci-plan: crossplatform detector returned '{other}'; failing closed");
                    true
                },
            }
        },
        Ok(output) => {
            eprintln!("ci-plan: crossplatform detector failed; failing closed");
            forward_detector_stderr(&output.stderr);
            true
        },
        Err(error) => {
            eprintln!("ci-plan: crossplatform detector failed; failing closed");
            eprintln!("  {error}");
            true
        },
    }
}

/// NON-diff events and malformed comparison SHAs fail closed to FULL scope.
fn early_scope_reason(event: &str, refs: &EventRefs) -> Option<&'static str> {
    if ["schedule", "workflow_dispatch"].contains(&event) {
        return Some("non-diff-event");
    }
    if !["pull_request", "merge_group", "push"].contains(&event) {
        return Some("unknown-event");
    }
    if refs.head.is_empty() || (refs.base.is_empty() && event != "push") {
        return Some(refs.missing);
    }
    if event == "push" && (refs.base.is_empty() || !valid_sha(&refs.base)) {
        return Some("push-before-zero-or-missing");
    }
    if !valid_sha(&refs.head) {
        return Some("head-not-a-40-hex-sha");
    }
    if !valid_sha(&refs.base) {
        return Some("base-not-a-40-hex-sha");
    }
    None
}

/// Emit the 15 path-aware pairs in the exact order CI consumes.
fn emit_path_plan(
    sink: &mut Sink,
    event: &str,
    refs: &EventRefs,
    kv: &BTreeMap<String, String>,
    run_code_jobs: &str,
    needs_crossplatform: bool,
) -> Result<(), String> {
    for (key, fallback) in [
        ("docs_only", "false"),
        ("ci_only", "false"),
        ("code_changed", "false"),
        ("all", "false"),
        ("affected", "false"),
    ] {
        sink.emit(key, kv_or(kv, key, fallback))?;
    }
    sink.emit("run_code_jobs", run_code_jobs)?;
    for (key, fallback) in [
        ("needs_ai", "true"),
        ("needs_mcp", "true"),
        ("needs_mutation_hotpath", "true"),
        ("snapshot_changed", "true"),
    ] {
        sink.emit(key, kv_or(kv, key, fallback))?;
    }
    let crossplatform = if needs_crossplatform { "true" } else { "false" };
    sink.emit("needs_crossplatform", crossplatform)?;
    sink.emit(
        "required_producers",
        if run_code_jobs == "true" {
            literal("FULL_PRODUCERS")
        } else {
            literal("NARROW_PRODUCERS")
        },
    )?;
    sink.emit("scope_reason", &format!("event={event} mode=path-aware"))?;
    sink.emit("base_sha", &refs.base)?;
    sink.emit("head_sha", &refs.head)?;
    eprintln!(
        "ci-plan: event={event} base={} head={} -> run_code_jobs={run_code_jobs} needs_crossplatform={crossplatform}",
        refs.base, refs.head
    );
    Ok(())
}

/// `git rev-parse --show-toplevel 2>/dev/null || pwd`.
fn repo_root() -> PathBuf {
    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            let text = String::from_utf8_lossy(&out.stdout);
            let path = text.trim();
            (!path.is_empty()).then(|| PathBuf::from(path))
        })
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default()
}

fn override_or_default(env_name: &str, root: &Path, script: &str) -> PathBuf {
    let override_value = env_var(env_name);
    if !override_value.is_empty() {
        return PathBuf::from(override_value);
    }
    root.join("scripts").join(script)
}

/// Bash's `${KV[key]:-default}`: an EMPTY value also takes the default.
fn kv_or<'a>(kv: &'a BTreeMap<String, String>, key: &str, default: &'a str) -> &'a str {
    match kv.get(key).map(String::as_str) {
        Some(value) if !value.is_empty() => value,
        _ => default,
    }
}

/// Bash's `while IFS='=' read -r key value` splits at the first `=`.
fn parse_kv(output: &str) -> BTreeMap<String, String> {
    let mut kv = BTreeMap::new();
    for line in output.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if !key.is_empty() {
                kv.insert(key.to_string(), value.to_string());
            }
        }
    }
    kv
}
