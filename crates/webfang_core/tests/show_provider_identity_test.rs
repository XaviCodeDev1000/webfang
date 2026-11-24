#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2106 — `show` and `doctor` must name what the run REALLY uses.
//!
//! The `[[model_profiles]]` layer describes the LOCAL ONNX models that
//! ADR-0004 step 6 retired; no run consumes it. Before this fix, `show`
//! printed `granite-97m` as "the profile of the run" and `doctor` repeated
//! the same id, so an operator asking "what will this run use?" got a
//! retired model back.
//!
//! These tests run the REAL binary (the same surface the operator sees) and
//! assert on stdout: the effective embeddings provider is the run's identity,
//! and the model-profile numbers survive only as labelled historical metadata.
//!
//! The unit-level contract lives in `cli/commands/show.rs`; this file pins
//! the rendered output, which is the only thing an operator can observe.

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

use common::cli_harness::{cmd, redact_nondeterministic};
use insta::assert_snapshot;

/// One embedding-capable provider: the identity a run would dial.
const KNOWN_PROVIDER_TOML: &str = "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

/// A provider whose `base_url` embeds a credential in the URL authority.
/// `alice:s3cr3t-do-not-print` is the leak marker; the endpoint after the `@`
/// must stay identifiable (S1 / issue #3006).
const USERINFO_PROVIDER_TOML: &str =
    "[[providers]]\nid = \"userinfo\"\ndisplay_name = \"Userinfo\"\n\
     kind = \"open_ai_compatible\"\n\
     base_url = \"https://alice:s3cr3t-do-not-print@api.example.test:8443/v1?key=public\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

/// A cannot-be-a-base URL has no authority for `set_username` / `set_password`
/// to clear, so credential-looking text can survive in its path. The
/// diagnostic must never print that text verbatim.
const OPAQUE_USERINFO_PROVIDER_TOML: &str =
    "[[providers]]\nid = \"opaque\"\ndisplay_name = \"Opaque\"\n\
     kind = \"open_ai_compatible\"\n\
     base_url = \"mailto:user:s3cr3t-do-not-print@host/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

/// A second provider, so the explicit-id path can be told apart from the
/// config-order default.
const OTHER_PROVIDER_TOML: &str = "[[providers]]\nid = \"other\"\ndisplay_name = \"Other\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://other.example.test/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text-v2\"\n";

/// Temp dir + `config.toml` seeded with `contents`. The dir is returned so it
/// outlives the test body.
fn seeded_config(contents: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, contents).expect("write config");
    (dir, config)
}

/// Run the `webfang` binary with `args` under `WEBFANG_CONFIG=config`.
fn run_with_config(args: &[&str], config: &Path) -> Output {
    cmd()
        .args(args)
        .env("WEBFANG_CONFIG", config)
        .output()
        .expect("run binary")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The part of the output that comes BEFORE the budget section.
///
/// The provider block is the run's identity and must never name a retired
/// local model. The budget block that follows legitimately names the default
/// profile (T2b), so the identity assertion has to stop at the budget header.
fn before_budget_section(stdout: &str) -> &str {
    stdout
        .split("=== show: budget efectivo ===")
        .next()
        .unwrap_or(stdout)
        .split("Budget del cleaner:")
        .next()
        .unwrap_or(stdout)
}

// ============================================================================
// show
// ============================================================================

/// `show` names the effective embeddings provider: the identity of the run.
#[test]
fn show_names_the_effective_embeddings_provider() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("=== show: provider de embeddings efectivo ==="),
        "show must title the effective-provider section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("id: known"),
        "show must name the resolved provider, got:\n{stdout}"
    );
    assert!(
        stdout.contains("base_url: https://api.example.test/v1"),
        "show must report what it read from the config, got:\n{stdout}"
    );
}

/// The defect itself: the retired local model is never presented as the
/// profile of the run.
#[test]
fn show_never_presents_a_retired_local_model_as_the_run_profile() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    let run_identity = before_budget_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "the run's identity must not name a retired local model, got:\n{run_identity}"
    );
    assert!(
        stdout.contains("metadata histórica"),
        "the profile numbers must survive as labelled metadata, got:\n{stdout}"
    );
    assert!(
        stdout.contains("not the identity of the run"),
        "the profile block must say it is not the run identity, got:\n{stdout}"
    );
    assert!(
        stdout.contains("max_chars: 98304"),
        "the printed budget must be the governing profile budget, got:\n{stdout}"
    );
}

/// An id the operator named is resolved BY ID, and the output says where it
/// came from.
#[test]
fn show_resolves_the_explicitly_named_provider() {
    let (_dir, config) = seeded_config(&format!("{KNOWN_PROVIDER_TOML}{OTHER_PROVIDER_TOML}"));

    let output = run_with_config(&["--embedding-provider", "other", "show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("id: other"),
        "show must resolve the named provider, got:\n{stdout}"
    );
    assert!(
        stdout.contains("origen: operatorio"),
        "show must say the provider came from the operator, got:\n{stdout}"
    );
}

/// An unknown id is a hard configuration error that names the id — and it
/// talks about a PROVIDER, never about a model profile: `--embedding-provider`
/// lives in the provider namespace (the namespace collision #2106 closes).
#[test]
fn show_with_an_unknown_provider_id_is_a_config_error_naming_the_id() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["--embedding-provider", "does-not-exist", "show"], &config);
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(78),
        "an unknown provider id is a configuration error (exit 78), got:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("does-not-exist"),
        "the error must name the unknown id, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("perfil de modelo"),
        "the error must not talk about a model profile — the id names a \
         provider, got:\n{stderr}"
    );
}

/// No configured provider: `show` says so. It does NOT fall back to a
/// shipped profile to fill the gap.
#[test]
fn show_without_any_provider_reports_no_embeddings_provider() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("sin provider de embeddings configurado"),
        "show must report the empty slot, got:\n{stdout}"
    );
    let run_identity = before_budget_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "an empty slot must not be filled with a retired local model, got:\n{run_identity}"
    );
}

// ============================================================================
// doctor
// ============================================================================

/// `doctor` keeps its diagnosis, and labels the profile numbers as history.
#[test]
fn doctor_labels_the_profile_numbers_as_historical_metadata() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["doctor"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("id: known"),
        "doctor must name the resolved provider, got:\n{stdout}"
    );
    // T2b: the profile now supplies the GOVERNING character budget, but it is
    // still not the run's identity (no endpoint is dialed from it).
    assert!(
        stdout.contains("now governs the cleaner"),
        "doctor must name the budget boundary the run actually consumes, got:\n{stdout}"
    );
    assert!(
        stdout.contains("not the identity of the run"),
        "doctor must say the profile is not the run identity, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--max-chars (o WEBFANG_MAX_CHARS): 98304"),
        "the printed budget must be the governing profile budget, got:\n{stdout}"
    );
    let run_identity = before_budget_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "the run's identity must not name a retired local model, got:\n{run_identity}"
    );
}

/// Two diagnostics, one truth: `show` and `doctor` must resolve the same
/// provider (they read the same config).
#[test]
fn show_and_doctor_agree_on_the_effective_provider() {
    let (_dir, config) = seeded_config(&format!("{KNOWN_PROVIDER_TOML}{OTHER_PROVIDER_TOML}"));

    let show_stdout = stdout_of(&run_with_config(&["show"], &config));
    let doctor_stdout = stdout_of(&run_with_config(&["doctor"], &config));

    assert!(
        show_stdout.contains("id: known"),
        "show must name the config-order default, got:\n{show_stdout}"
    );
    assert!(
        doctor_stdout.contains("id: known"),
        "doctor must name the same provider, got:\n{doctor_stdout}"
    );
}

// ============================================================================
// credential redaction (S1 / issue #3006)
// ============================================================================

/// The shared provider block a command renders through `print_provider_block`.
///
/// Both `show` and `doctor` emit it; slicing it out keeps the snapshot focused
/// on the diagnostic surface under test and away from `doctor`'s reachability
/// prose, which is a different code path.
fn provider_block_of(stdout: &str) -> String {
    let body = stdout
        .split_once("Provider configurado:")
        .map(|(_, rest)| rest)
        .expect("provider block must be rendered");
    let mut lines = vec!["Provider configurado:".to_string()];
    lines.extend(
        body.lines()
            .skip_while(|line| line.trim().is_empty())
            .take_while(|line| line.starts_with("  "))
            .map(str::to_string),
    );
    lines.join("\n")
}

/// A credential embedded in a provider `base_url` must never be rendered by
/// the `doctor` / `show` diagnostics, while the endpoint (scheme, host, port,
/// path, query) stays identifiable. This is a diagnostic-render fix only:
/// config load and request egress keep the configured URL unchanged.
#[test]
fn show_and_doctor_redact_provider_base_url_userinfo() {
    let (dir, config) = seeded_config(USERINFO_PROVIDER_TOML);

    let show = provider_block_of(&stdout_of(&run_with_config(&["show"], &config)));
    let doctor = provider_block_of(&stdout_of(&run_with_config(&["doctor"], &config)));
    let rendered = format!("--- show ---\n{show}\n--- doctor ---\n{doctor}\n");

    assert_snapshot!(
        "provider_base_url_userinfo_is_redacted",
        redact_nondeterministic(dir.path(), &rendered)
    );
}

/// A provider URL whose authority cannot be edited must not leak its raw text
/// into `show` / `doctor`: a failed redaction must never fail open (#3006).
#[test]
fn show_and_doctor_never_print_unredactable_base_url_text() {
    let (dir, config) = seeded_config(OPAQUE_USERINFO_PROVIDER_TOML);

    let show = provider_block_of(&stdout_of(&run_with_config(&["show"], &config)));
    let doctor = provider_block_of(&stdout_of(&run_with_config(&["doctor"], &config)));
    let rendered = format!("--- show ---\n{show}\n--- doctor ---\n{doctor}\n");

    assert!(
        !rendered.contains("s3cr3t-do-not-print"),
        "credential-looking text must not reach diagnostic output: {rendered}"
    );
    assert_snapshot!(
        "provider_base_url_unredactable_is_not_printed",
        redact_nondeterministic(dir.path(), &rendered)
    );
}
