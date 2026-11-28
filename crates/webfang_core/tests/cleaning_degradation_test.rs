#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! T6 / issue #3117 (finding E9, decision D1) — semantic-cleaning degradation
//! must be VISIBLE. Every assertion here is made from the outside, through the
//! real binary and the artefacts it writes, so the marker cannot drift away
//! from the run that produced it:
//!
//! 1. **Marker in the export.** The JSONL line of a page whose text fell back
//!    to the raw content carries `degraded_reason` naming the variant; a page
//!    that cleaned normally carries no such field (additive, optional).
//! 2. **Marker in the `RecordStore`.** The same fact is durable in the store
//!    the run wrote, so a later `--resume` can tell "cleaned" from "fell back".
//! 3. **The right message per variant.** stderr names `ChunkTooLarge` instead
//!    of the old fixed sentence that claimed a character limit for every
//!    cause, and the run closes with one tally line.
//! 4. **`--strict-clean-ai`.** The only way to turn a partial degradation into
//!    exit 78; the default stays exit 0 with the marker present.
//! 5. **Egress notice (D1).** One stderr warning at `--clean-ai` startup naming
//!    the embeddings endpoint HOST only — and nothing when `--clean-ai` is
//!    absent, even though the provider is still resolved for vault search.
//!
//! Why only `ChunkTooLarge` is driven through the binary: the shipped cleaner
//! (`SemanticCleanerImpl`) produces exactly one `DomainRecoverable` variant
//! from page text — a single paragraph that exceeds `--max-chars`. `Tokenize`
//! and `InvalidSpan` are unreachable from the binary, so their end-to-end
//! coverage lives at the pipeline level in
//! `crates/webfang_core/src/cli/export_flow_tests.rs`, with the same three
//! observable effects (JSONL, record store, summary line).

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::cleaning_site::{mount_cleaning_site, CLEAN_PATH, DEGRADED_PATH, MAX_CHARS};
use common::cli_harness::{
    assert_snapshot_redacted, spawn_budgeted, std_scraper_cmd, BehavioralTest,
};

/// The field the marker travels in, asserted through the JSON artefacts only:
/// the test must fail when the writer renames it, so the name is spelled out
/// here the same way a downstream consumer reads it.
const MARKER_KEY: &str = "degraded_reason";

/// The variant name the shipped cleaner can produce from page text.
const CHUNK_TOO_LARGE: &str = "ChunkTooLarge";

/// The sentence the old fixed warning printed for ALL three variants. Its
/// survival is the defect: it named one cause and hid the other two.
const OLD_BLANKET_WARNING: &str = "chunk exceeds character limit";

/// The tally line every degraded run closes with.
const SUMMARY_PREFIX: &str = "Semantic cleaning: ";

/// The egress notice's counting anchor — the constant the binary PRINTS, not a
/// copy of it, so a reworded notice cannot leave this test asserting on a
/// string nobody emits.
const NOTICE_MARKER: &str =
    webfang_core::cli::clean_ai_egress_notice::CLEAN_AI_EGRESS_NOTICE_MARKER;

/// Pages the fixture crawl exports: the hub plus both leaves.
const PAGES: usize = 3;

/// Every path one test owns, under a single temp root so the snapshots carry
/// one redacted prefix instead of several machine paths.
struct Scratch {
    out: PathBuf,
    state: PathBuf,
}

impl Scratch {
    fn new(root: &Path) -> Self {
        Self {
            out: root.join("out"),
            state: root.join("state"),
        }
    }
}

/// Run the real binary against the cleaning fixture with the flags every leg
/// shares: one worker (deterministic crawl order), no pacing, the provider
/// configured, and `--quiet` so the snapshot holds the WARN lines this issue
/// is about instead of the whole INFO narration.
fn scraper(t: &BehavioralTest, scratch: &Scratch, provider_config: &Path) -> std::process::Command {
    let mut cmd = std_scraper_cmd(&t.server.uri(), &scratch.out);
    cmd.env("WEBFANG_CONFIG", provider_config)
        .arg("--embedding-provider")
        .arg(common::cli_harness::embeddings::PROVIDER_ID)
        .arg("--clean-ai")
        .arg("--quiet")
        .arg("--resume")
        .arg("--state-dir")
        .arg(&scratch.state)
        .arg("--concurrency")
        .arg("1")
        .arg("--max-depth")
        .arg("1")
        .arg("--max-pages")
        .arg(PAGES.to_string())
        .arg("--delay-ms")
        .arg("0")
        .arg("--ignore-robots")
        .arg("--max-chars")
        .arg(MAX_CHARS);
    cmd
}

/// The JSONL records the run wrote, keyed by URL.
fn exported_records(out: &Path) -> serde_json::Map<String, serde_json::Value> {
    let path = out.join("export.jsonl");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("every JSONL line is valid JSON");
            let url = value["url"]
                .as_str()
                .expect("every record carries its url")
                .to_string();
            (url, value)
        })
        .collect()
}

/// The record objects of the run's `RecordStore`, keyed by canonical URL.
fn stored_records(state: &Path) -> serde_json::Map<String, serde_json::Value> {
    // The store is keyed by domain (the mock binds 127.0.0.1, no port).
    let path = state.join("127.0.0.1.json");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let value: serde_json::Value = serde_json::from_str(&body).expect("store is valid JSON");
    value["records"]
        .as_object()
        .expect("the envelope carries its records")
        .clone()
}

/// The single store record whose canonical URL ends with `suffix`.
fn stored_record_ending_with<'a>(
    records: &'a serde_json::Map<String, serde_json::Value>,
    suffix: &str,
) -> &'a serde_json::Value {
    let mut found = records
        .iter()
        .filter(|(url, _)| url.ends_with(suffix))
        .map(|(_, record)| record);
    let record = found
        .next()
        .unwrap_or_else(|| panic!("no store record for a URL ending with {suffix}: {records:?}"));
    assert!(
        found.next().is_none(),
        "exactly one store record may end with {suffix}"
    );
    record
}

/// Every line of `stderr` that mentions `needle`.
fn lines_with<'a>(stderr: &'a str, needle: &str) -> Vec<&'a str> {
    stderr
        .lines()
        .filter(|line| line.contains(needle))
        .collect()
}

/// The fixture every leg starts from: the cleaning site mounted, a fake
/// REMOTE embeddings provider configured, and the temp paths of the run.
///
/// One arrange helper, not three copies: the three tests differ in the FLAGS
/// they pass and in what they assert, never in how the site is built.
async fn arrange(t: &BehavioralTest) -> (Scratch, std::path::PathBuf) {
    mount_cleaning_site(&t.server).await;
    common::cli_harness::embeddings::mount_embeddings(&t.server).await;
    let provider_config = common::cli_harness::embeddings::provider_config(&t.server);
    (Scratch::new(t.out.path()), provider_config)
}

/// Items 1–4: a run with ONE degraded page keeps exit 0, writes the marker to
/// the JSONL line AND to the record store, and names the cause it actually
/// received instead of the old blanket sentence.
#[tokio::test]
async fn degraded_page_carries_its_cause_in_jsonl_store_and_summary() {
    let t = BehavioralTest::new().await;
    let (scratch, provider_config) = arrange(&t).await;

    let output = spawn_budgeted(
        "degraded_page_carries_its_cause_in_jsonl_store_and_summary",
        scraper(&t, &scratch, &provider_config),
        None,
    )
    .await;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a PARTIAL degradation keeps today's exit code; stderr:\n{stderr}"
    );

    // 1. The export carries the marker on the degraded page only.
    let exported = exported_records(&scratch.out);
    let degraded = exported
        .iter()
        .find(|(url, _)| url.ends_with(DEGRADED_PATH))
        .map(|(_, record)| record)
        .unwrap_or_else(|| panic!("no exported line for {DEGRADED_PATH}: {exported:?}"));
    assert_eq!(
        degraded[MARKER_KEY], CHUNK_TOO_LARGE,
        "the exported line must state that the page fell back to raw text, and why"
    );
    let clean = exported
        .iter()
        .find(|(url, _)| url.ends_with(CLEAN_PATH))
        .map(|(_, record)| record)
        .unwrap_or_else(|| panic!("no exported line for {CLEAN_PATH}: {exported:?}"));
    assert!(
        clean.get(MARKER_KEY).is_none(),
        "a page that cleaned normally must not carry the marker: {clean}"
    );

    // 2. The same fact is durable in the record store.
    let stored = stored_records(&scratch.state);
    assert_eq!(
        stored_record_ending_with(&stored, DEGRADED_PATH)[MARKER_KEY],
        CHUNK_TOO_LARGE,
        "the durable record must distinguish 'cleaned' from 'fell back to raw text'"
    );
    assert!(
        stored_record_ending_with(&stored, CLEAN_PATH)
            .get(MARKER_KEY)
            .is_none(),
        "a cleaned record must not claim a degradation"
    );

    // 3. One WARN per degradation, naming the variant it actually received.
    assert_eq!(
        lines_with(&stderr, DEGRADED_PATH).len(),
        1,
        "exactly one line may name the degraded page; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(CHUNK_TOO_LARGE),
        "the warning must name the variant it received; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(OLD_BLANKET_WARNING),
        "the old one-cause-for-everything sentence must be gone; stderr:\n{stderr}"
    );

    // 4. One tally line closes the run.
    let summaries = lines_with(&stderr, SUMMARY_PREFIX);
    assert_eq!(
        summaries.len(),
        1,
        "the run must close with exactly one tally line; stderr:\n{stderr}"
    );
    assert!(
        summaries[0].contains(&format!("1/{PAGES} degraded")),
        "the tally must state how many pages degraded: {}",
        summaries[0]
    );
    assert!(
        summaries[0].contains(&format!("{CHUNK_TOO_LARGE}: 1")),
        "the tally must carry the per-variant breakdown: {}",
        summaries[0]
    );

    assert_snapshot_redacted(
        "degraded_page_carries_its_cause_in_jsonl_store_and_summary_stderr",
        t.out.path(),
        stderr.to_string(),
    );
}

/// Item 5: `--strict-clean-ai` is the ONLY way a partial degradation becomes
/// exit 78, and a strict run hands back no degraded artifact.
#[tokio::test]
async fn strict_clean_ai_turns_one_degraded_page_into_exit_78() {
    let t = BehavioralTest::new().await;
    let (scratch, provider_config) = arrange(&t).await;

    let mut strict = scraper(&t, &scratch, &provider_config);
    strict.arg("--strict-clean-ai");
    let output = spawn_budgeted(
        "strict_clean_ai_turns_one_degraded_page_into_exit_78",
        strict,
        None,
    )
    .await;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(78),
        "--strict-clean-ai must fail the run on a partial degradation; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("--strict-clean-ai"),
        "the failure must name the flag that caused it; stderr:\n{stderr}"
    );
    assert!(
        lines_with(&stderr, SUMMARY_PREFIX).len() == 1,
        "a strict run still owes the operator the tally; stderr:\n{stderr}"
    );
    assert!(
        !scratch.out.join("export.jsonl").exists(),
        "a strict run must not hand back the degraded artifact it refuses"
    );

    assert_snapshot_redacted(
        "strict_clean_ai_turns_one_degraded_page_into_exit_78_stderr",
        t.out.path(),
        stderr.to_string(),
    );
}

/// Item 6 (D1): one notice per `--clean-ai` run naming the endpoint HOST —
/// never the full URL, never credentials — and silence when the operator never
/// asked for semantic cleaning, even though the provider was still resolved.
#[tokio::test]
async fn egress_notice_names_only_the_endpoint_host_and_only_with_clean_ai() {
    let t = BehavioralTest::new().await;
    let (scratch, provider_config) = arrange(&t).await;

    let output = spawn_budgeted(
        "egress_notice_names_only_the_endpoint_host_and_only_with_clean_ai",
        scraper(&t, &scratch, &provider_config),
        None,
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let notice = NOTICE_MARKER;
    let notices = lines_with(&stderr, notice);
    assert_eq!(
        notices.len(),
        1,
        "a --clean-ai run owes exactly ONE egress notice; stderr:\n{stderr}"
    );
    assert!(
        notices[0].contains("127.0.0.1"),
        "the notice must name the endpoint host: {}",
        notices[0]
    );
    assert!(
        !notices[0].contains("/v1"),
        "the notice must name the HOST only, never the full URL: {}",
        notices[0]
    );

    // The same site, the same configured provider, no `--clean-ai`: the
    // provider is still resolved (and probed) for vault search, so this leg
    // proves the notice is gated on semantic cleaning, not on provider
    // resolution.
    let plain_scratch = Scratch::new(&t.out.path().join("no-clean-ai"));
    std::fs::create_dir_all(&plain_scratch.out).expect("create the second output dir");
    let mut cmd = std_scraper_cmd(&t.server.uri(), &plain_scratch.out);
    cmd.env("WEBFANG_CONFIG", &provider_config)
        .arg("--embedding-provider")
        .arg(common::cli_harness::embeddings::PROVIDER_ID)
        .arg("--quiet")
        .arg("--ignore-robots")
        .arg("--delay-ms")
        .arg("0")
        .arg("--max-pages")
        .arg(PAGES.to_string());
    let plain = spawn_budgeted("egress_notice_absent_without_clean_ai", cmd, None).await;
    let plain_stderr = String::from_utf8_lossy(&plain.stderr);
    assert!(
        !plain_stderr.contains(notice),
        "a run without --clean-ai must emit no egress notice; stderr:\n{plain_stderr}"
    );

    assert_snapshot_redacted(
        "egress_notice_with_clean_ai_stderr",
        t.out.path(),
        stderr.to_string(),
    );
}
