//! Pipeline-level coverage for the three semantic-cleaning degradation
//! variants (T6, issue #3117 / finding E9).
//!
//! `ChunkTooLarge` is reachable from the real binary and is pinned there
//! (`crates/webfang_core/tests/cleaning_degradation_test.rs`). `Tokenize` and
//! `InvalidSpan` are NOT reachable from it: the shipped cleaner
//! (`SemanticCleanerImpl`) fails only when a single paragraph exceeds the
//! character budget — it never tokenizes and never builds a `start > end`
//! span — so no page text can produce those two. Driving them through a
//! scripted [`SemanticCleaner`] is therefore the only honest way to prove
//! their marker, and the assertions are the same three OBSERVABLE effects the
//! binary test makes: the JSONL line, the durable `RecordStore` record and the
//! one summary line. Nothing here reads internal state.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use super::{run_export, test_scraped as page, ExportConfig};
use crate::domain::semantic_cleaner::{private::Sealed, SemanticCleaner};
use crate::domain::DocumentChunk;
use crate::error::SemanticError;
use crate::infrastructure::observability::test_log_capture::LogBuffer;

/// The only port the export flow needs from the state store: the path the
/// durable record is written to.
struct TempStateStore {
    path: PathBuf,
}

impl crate::domain::persistence::StateStorePort for TempStateStore {
    fn get_state_path(&self) -> PathBuf {
        self.path.clone()
    }
}

/// The failure the scripted cleaner reports for one URL.
///
/// A copyable script instead of a stored `SemanticError` (`SemanticError` is
/// not `Clone`): the future rebuilds the variant at call time, so the cleaner
/// holds no error value it would have to duplicate.
#[derive(Clone, Copy)]
enum FailScript {
    /// A paragraph above the character budget.
    ChunkTooLarge,
    /// Text the cleaner could not segment at all.
    Tokenize,
    /// A chunker bug: a byte span whose start is past its end.
    InvalidSpan,
}

impl FailScript {
    /// The variant name the artefacts must carry.
    const fn variant(self) -> &'static str {
        match self {
            Self::ChunkTooLarge => "ChunkTooLarge",
            Self::Tokenize => "Tokenize",
            Self::InvalidSpan => "InvalidSpan",
        }
    }

    /// The error this script reports.
    fn error(self) -> SemanticError {
        match self {
            Self::ChunkTooLarge => SemanticError::ChunkTooLarge {
                chunk_id: "0".to_string(),
                chars: 900,
                max: 512,
            },
            Self::Tokenize => SemanticError::Tokenize("empty text".to_string()),
            Self::InvalidSpan => SemanticError::InvalidSpan { start: 9, end: 3 },
        }
    }
}

/// Cleaner whose per-URL behaviour is scripted: `fail_url` fails with
/// `script`, every other page produces one chunk.
struct ScriptedCleaner {
    fail_url: String,
    script: FailScript,
}

impl Sealed for ScriptedCleaner {}

impl SemanticCleaner for ScriptedCleaner {
    fn clean<'a>(
        &'a self,
        url: &'a str,
        _html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>> {
        let script = self.script;
        let fails = self.fail_url == url;
        Box::pin(async move {
            if fails {
                return Err(script.error());
            }
            Ok(vec![DocumentChunk::new(
                uuid::Uuid::new_v4(),
                "",
                "",
                "cleaned text for the healthy page",
            )])
        })
    }

    fn max_chars(&self) -> usize {
        512
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// The page the scripted cleaner cleans normally.
const HEALTHY: &str = "https://variant.test/healthy";

/// The page the scripted cleaner degrades with the variant under test.
const DEGRADED: &str = "https://variant.test/degraded";

/// The exported record for `url`, from the JSONL lines the run wrote.
fn exported_record<'a>(records: &'a [serde_json::Value], url: &str) -> &'a serde_json::Value {
    records
        .iter()
        .find(|record| record["url"] == url)
        .unwrap_or_else(|| panic!("no exported line for {url}: {records:?}"))
}

/// The durable record whose canonical URL ends with `suffix`.
fn stored_record<'a>(
    records: &'a serde_json::Map<String, serde_json::Value>,
    suffix: &str,
) -> &'a serde_json::Value {
    records
        .iter()
        .find(|(url, _)| url.ends_with(suffix))
        .map(|(_, record)| record)
        .unwrap_or_else(|| panic!("no durable record ending with {suffix}: {records:?}"))
}

/// Effect 1: the JSONL line of the degraded page carries the variant, and the
/// page that cleaned normally carries nothing.
fn assert_exported_marker(out_dir: &std::path::Path, variant: &str) {
    let jsonl = std::fs::read_to_string(out_dir.join("export.jsonl")).expect("export.jsonl");
    let exported: Vec<serde_json::Value> = jsonl
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid JSONL line"))
        .collect();
    assert_eq!(
        exported_record(&exported, DEGRADED)["degraded_reason"],
        variant,
        "{variant}: the exported line must carry the variant that degraded it"
    );
    assert!(
        exported_record(&exported, HEALTHY)
            .get("degraded_reason")
            .is_none(),
        "{variant}: a page that cleaned normally must not carry the marker"
    );
}

/// Effect 2: the same fact is durable in the `RecordStore`.
fn assert_stored_marker(store_path: &std::path::Path, variant: &str) {
    let body = std::fs::read_to_string(store_path).expect("the run must persist its record store");
    let value: serde_json::Value = serde_json::from_str(&body).expect("valid store json");
    let records = value["records"]
        .as_object()
        .expect("the envelope carries its records");
    assert_eq!(
        stored_record(records, "/degraded")["degraded_reason"],
        variant,
        "{variant}: the durable record must state the degradation"
    );
    assert!(
        stored_record(records, "/healthy")
            .get("degraded_reason")
            .is_none(),
        "{variant}: a cleaned record must not claim a degradation"
    );
}

/// Effect 3: one tally line with the breakdown, and the old blanket sentence
/// (one cause for all three variants) gone.
fn assert_summary_line(log: &str, variant: &str) {
    let summaries: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("Semantic cleaning: "))
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "{variant}: exactly one tally line; log:\n{log}"
    );
    assert!(
        summaries[0].contains(&format!("1/2 degraded ({variant}: 1)")),
        "{variant}: the tally must state the count and the cause: {}",
        summaries[0]
    );
    assert!(
        !log.contains("chunk exceeds character limit"),
        "{variant}: the old one-cause-for-everything warning must be gone; log:\n{log}"
    );
}

/// Drive one degraded variant through the REAL export pipeline and assert the
/// three observable effects.
fn assert_variant_is_visible(script: FailScript) {
    let variant = script.variant();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let out_dir = tmp.path().join("out");
    let store = TempStoreHolder::new(&tmp.path().join("state"));
    let config = ExportConfig {
        results: Arc::from(vec![
            page(HEALTHY, "Healthy", "text the cleaner will keep"),
            page(DEGRADED, "Degraded", "text the cleaner will drop"),
        ]),
        output_dir: out_dir.clone(),
        format: crate::OutputFormat::default(),
        export_format: crate::ExportFormat::Jsonl,
        clean_ai: true,
        quick_save: false,
        vault_path: None,
        obsidian_options: Default::default(),
        state_store: Some(&store.state_store),
        resume: false,
        cancel: None,
        ai_threshold: 0.3,
        ai_max_chars: 512,
        ai_offline: false,
        ai_strict: false,
    };
    let cleaner: Arc<dyn SemanticCleaner> = Arc::new(ScriptedCleaner {
        fail_url: DEGRADED.to_string(),
        script,
    });

    // The summary line is a `tracing` event, so the capture must wrap the run.
    let buffer = LogBuffer::run(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime
            .block_on(run_export(config, Some(cleaner)))
            .expect("one degraded page out of two is not a run failure");
    });

    assert_exported_marker(&out_dir, variant);
    assert_stored_marker(&store.path().join("variant.test.json"), variant);
    assert_summary_line(&buffer.text(), variant);
}

/// The state-store stub plus the directory it writes into, so the test can
/// read the artefact back.
struct TempStoreHolder {
    state_store: TempStateStore,
}

impl TempStoreHolder {
    fn new(dir: &std::path::Path) -> Self {
        std::fs::create_dir_all(dir).expect("create state dir");
        Self {
            state_store: TempStateStore {
                path: dir.join("variant.test.json"),
            },
        }
    }

    fn path(&self) -> &std::path::Path {
        self.state_store.path.parent().expect("state dir")
    }
}

#[test]
fn chunk_too_large_is_visible_in_jsonl_store_and_summary() {
    assert_variant_is_visible(FailScript::ChunkTooLarge);
}

#[test]
fn tokenize_is_visible_in_jsonl_store_and_summary() {
    assert_variant_is_visible(FailScript::Tokenize);
}

#[test]
fn invalid_span_is_visible_in_jsonl_store_and_summary() {
    assert_variant_is_visible(FailScript::InvalidSpan);
}
