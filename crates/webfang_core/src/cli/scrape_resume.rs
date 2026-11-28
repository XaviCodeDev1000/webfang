//! Resume/state wiring for the CLI scrape flow: the default state
//! directory, the `PersistenceMode` resume gate, the unreadable-state
//! notice and the v2 `RecordStore` bridge (ADR-0010 composition edge).
//!
//! Split from `scrape_flow.rs`; every item is re-exported there so the
//! external full-path callers (`cli::orchestrator`, `export_flow`,
//! `batch_flow`, integration tests) keep resolving unchanged.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};
use url::Url;

use crate::application::container;
use crate::application::export_factory;
use crate::application::resume::filter_committed;
use crate::cli::error::CliExit;
use crate::domain::page_state::{Committed, Stateful};
use crate::domain::persistence::{
    DomainRecords, PersistenceMode, RawRecord, RecordStoreError, StateStorePort,
};
use crate::domain::CorrelationId;
use crate::infrastructure::export::RecordStore;

/// Resolve the default state directory (`<cache>/webfang/state` —
/// XDG_CACHE_HOME or `~/.cache` on Linux; %LOCALAPPDATA% / ~/Library/Caches
/// elsewhere).
///
/// Pure helper extracted for `PersistenceMode::from_config` callers. The
/// base comes from the single platform-paths helper (XP-F-05, #1608); the
/// fallback keeps this site's previous fail-soft behavior when no user home
/// exists.
#[must_use]
pub fn resolve_default_state_dir() -> PathBuf {
    // S17: a tenant-or-run scope keeps checkpoints under its own root; an
    // explicit `--state-dir` / persistence override is resolved by the caller
    // and never reaches this default.
    if let Some(scoped) = crate::domain::paths::TenantScope::current().state_dir() {
        return scoped;
    }
    crate::domain::paths::cache_base_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("webfang")
        .join("state")
}

/// Compare paths without requiring them to exist: `std::path::absolute`
/// resolves the CWD against a relative path, while `canonicalize` would fail
/// on a directory that is not there yet.
fn normalize(path: &std::path::Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The single DIRECTORY every proven-COMMITTED record was written into, when
/// they agree on exactly one.
///
/// `None` when there is no committed record, when a committed record carries
/// no `output_location`, or when the committed records disagree: a mixed
/// history is not evidence of a mismatch, so the caller fails OPEN (no
/// refusal) rather than inventing one.
fn recorded_output_root(records: &DomainRecords) -> Option<PathBuf> {
    let mut recorded: Option<PathBuf> = None;
    for record in records.values() {
        // The same rule as `application::resume::is_committed_proven` — that
        // helper is private to the application layer and takes a URL, while
        // this loop already holds the record. Re-expressed here WITHOUT
        // copying its lookup: the proof is the domain type-level transition
        // (`Stateful<_, Committed>::reconcile`, the D2 invariant table), not a
        // status comparison, so the two cannot drift into different verdicts
        // about what "committed" means.
        if Stateful::<RawRecord, Committed>::reconcile(record.clone()).is_err() {
            continue;
        }
        let location = record.output_location.as_deref()?;
        let parent = std::path::Path::new(location).parent()?;
        match &recorded {
            Some(seen) if normalize(seen) != normalize(parent) => return None,
            Some(_) => {},
            None => recorded = Some(normalize(parent)),
        }
    }
    recorded
}

/// Refuse a resume whose recorded output directory is neither of the two this
/// run would write into (#3097 / T4).
///
/// Before this gate a resume into a different `--output` exited **0** and
/// wrote nothing: the committed records short-circuited the export, so the
/// operator got an empty directory and a success code. A mismatch is now a
/// usage error (exit 64) naming both directories and the remedy.
fn guard_recorded_output_root(
    records: &DomainRecords,
    persistence_root: &std::path::Path,
    export_dir: &std::path::Path,
) -> Result<(), CliExit> {
    let Some(recorded) = recorded_output_root(records) else {
        return Ok(());
    };
    let matches_run = normalize(&recorded) == normalize(persistence_root)
        || normalize(&recorded) == normalize(export_dir);
    if matches_run {
        return Ok(());
    }
    Err(CliExit::UsageError(format!(
        "resuming into a different output directory: the previous run committed its pages to '{}', \
         but this run would write to '{}' (persistence root) and '{}' (export directory). \
         Resume into the recorded output, or start a fresh state (a new --state-dir, or remove the \
         state file).",
        recorded.display(),
        persistence_root.display(),
        export_dir.display(),
    )))
}

/// Apply resume mode filtering via `PersistenceMode`.
///
/// `mode` is the unified control-plane — exhaustive `match` on
/// `Disabled|Resume|Checkpoint|Full`. Only `Resume` and `Full` create a
/// state store and filter committed URLs; `Checkpoint` and `Disabled` pass
/// all URLs through.
///
/// # Errors
///
/// [`CliExit::IoError`] when the resume store is unreadable: corruption or
/// an unsupported version is an operator-visible failure, never a silent
/// fresh start that would re-drive committed work. [`CliExit::UsageError`]
/// when the records were committed into a different output directory than the
/// one this run would write into (#3097 / T4).
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    mode: &PersistenceMode,
    target_url: &str,
    _root_correlation: &CorrelationId,
    persistence_root: &std::path::Path,
    export_dir: &std::path::Path,
) -> Result<(Vec<Url>, Option<Arc<dyn StateStorePort>>), CliExit> {
    let state_store: Option<Arc<dyn StateStorePort>> = match mode {
        PersistenceMode::Disabled | PersistenceMode::Checkpoint { .. } => None,
        PersistenceMode::Resume { dir }
        | PersistenceMode::Full {
            resume_dir: dir, ..
        } => {
            info!("Resume mode enabled - tracking processed URLs");
            let domain = export_factory::domain_from_url(target_url);
            info!("State store domain: {}", domain);
            // Lazy and infallible: `build_state_store` performs no I/O — the
            // state directory is created later, on `save`. The former
            // `CliExit::IoError` creation-failure branch was unreachable dead
            // code (pinned by
            // `test_build_state_store_returns_ok_even_when_state_dir_is_a_file`).
            Some(container::build_state_store(dir.clone(), &domain))
        },
    };

    let filtered = match (state_store.as_ref(), mode.is_resume()) {
        (Some(store), true) => {
            let record_store = record_store_bridge(store.as_ref());
            // Fail fast, before the export can write anything: the historical
            // silent failure was a resume into a different `--output` that
            // exited 0 with an empty result.
            //
            // TWO reads of the state file, ONE decision, on the resume path
            // only: this load feeds the output-mismatch gate, and
            // `filter_committed` below loads again because that is its
            // contract (`application::resume` owns the single resume gate and
            // takes the store, not a record map). Consolidating the two reads
            // belongs THERE, in `application/resume.rs` — not in this gate,
            // which must not become a second implementation of the filter.
            let records = record_store
                .load_or_init()
                .map_err(|e| CliExit::IoError(e.to_string()))?;
            guard_recorded_output_root(&records, persistence_root, export_dir)?;
            filter_committed(urls_to_scrape, &record_store)
                .map_err(|e| CliExit::IoError(e.to_string()))?
                .0
        },
        _ => urls_to_scrape,
    };

    crate::cli::crash_points::hit(crate::cli::crash_points::PRE_FIRST_PERSIST);

    Ok((filtered, state_store))
}

/// P8-3 — tell the user, in Spanish, when the resume state file cannot be
/// read, BEFORE the resume gate treats it as empty.
///
/// The fresh-start policy itself is deliberate and pinned by
/// `resume_test::corrupt_state_falls_back_to_full_scrape`; what was missing is
/// that the only signal was an English `tracing::WARN`, so a user watched the
/// tool silently discard their persisted state and re-do all the work. AGENTS.md
/// reserves English for internal logs: user-facing text is Spanish.
///
/// Shared with `export_flow` (#1587): both CLI resume paths surface the same
/// notice. The notice states the consequence (restart from zero, prior work
/// re-runs) AND the remedy (inspect, fix or delete the named file before
/// retrying with `--resume`), not just the symptom.
///
/// Never fails and never changes the run's outcome; the original bytes stay on
/// disk (`load_or_init` preserves them) and the message names the path so the
/// user can inspect them.
pub(crate) fn warn_unreadable_state(store: &RecordStore) {
    let (symptom, path) = match store.load() {
        Ok(_) => return,
        Err(RecordStoreError::Corrupt { path }) => {
            ("está corrupto o no es JSON válido".to_string(), path)
        },
        Err(RecordStoreError::UnsupportedVersion { path, found }) => (
            format!("pertenece a una versión no soportada ({found})"),
            path,
        ),
        Err(err @ (RecordStoreError::Io { .. } | RecordStoreError::Backup { .. })) => {
            // Internal detail stays in the log; the user gets the plain fact.
            warn!(error = %err, "resume state file unreadable");
            ("no se pudo leer".to_string(), store.state_path())
        },
        // Writer-side rejection: not reachable from a read, matched so a new
        // error variant can never fall through into silence (fail-closed).
        Err(err @ RecordStoreError::InvalidRecord { .. }) => {
            warn!(error = %err, "resume state file rejected");
            ("fue rechazado por inválido".to_string(), store.state_path())
        },
    };
    eprintln!(
        "Advertencia: el archivo de estado {} {}. Se reanuda desde cero y el trabajo previo se repetirá; el original se conserva para inspección — corríjalo o elimínelo antes de reintentar con --resume.",
        path.display(),
        symptom,
    );
}

/// Bridge a state-store port handle onto the v2 `RecordStore` seam:
/// same directory + domain, so a legacy v1 state file migrates in place
/// on first load (Gate 2 policy lives inside `RecordStore`).
///
/// Lives in `cli` (ADR-0010): the composition edge constructs infrastructure
/// concretes; `application` consumes them through `RecordStorePort` only.
/// Shared with `export_flow` so both resume paths derive the same bridge.
pub(crate) fn record_store_bridge(state_store: &dyn StateStorePort) -> RecordStore {
    let path = state_store.get_state_path();
    let dir = path.parent().map_or_else(
        || std::path::PathBuf::from("."),
        std::path::Path::to_path_buf,
    );
    let domain = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("unknown")
        .to_string();
    RecordStore::new(domain).with_state_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::page_state::PageStatus;

    /// A record shaped so its `Committed` status is PROVEN by the D2 table:
    /// output location, content hash, no last error, at least one attempt.
    fn committed(url: &str, output_location: &str) -> RawRecord {
        RawRecord {
            url: url.to_string(),
            canonical_url: url.to_string(),
            run_id: "run-1".to_string(),
            content_hash: Some("hash".to_string()),
            attempts: 1,
            status: PageStatus::Committed,
            last_error: None,
            output_location: Some(output_location.to_string()),
            updated_at: 1,
        }
    }

    fn records(entries: Vec<RawRecord>) -> DomainRecords {
        entries
            .into_iter()
            .map(|record| (record.canonical_url.clone(), record))
            .collect()
    }

    /// The recorded root is the parent directory of the per-page FILE path a
    /// committed record stores (`<root>/<page>.md`).
    #[test]
    fn recorded_output_root_is_the_parent_of_the_committed_files() {
        let store = records(vec![
            committed("https://site.test/", "run-a/site.test/index.md"),
            committed("https://site.test/a", "run-a/site.test/a.md"),
        ]);
        assert_eq!(
            recorded_output_root(&store),
            Some(normalize(std::path::Path::new("run-a/site.test")))
        );
    }

    /// A record that does not reconstruct as `Committed` proves nothing about
    /// where the run wrote, so it never contributes a directory.
    #[test]
    fn recorded_output_root_ignores_unproven_records() {
        let mut exported = committed("https://site.test/a", "run-b/site.test/a.md");
        exported.status = PageStatus::Exported;
        let store = records(vec![
            exported,
            committed("https://site.test/", "run-a/site.test/index.md"),
        ]);
        assert_eq!(
            recorded_output_root(&store),
            Some(normalize(std::path::Path::new("run-a/site.test"))),
            "an EXPORTED record must not make the history look mixed"
        );
    }

    /// A mixed history is not evidence of a mismatch: fail OPEN.
    #[test]
    fn recorded_output_root_fails_open_when_parents_disagree() {
        let store = records(vec![
            committed("https://site.test/", "run-a/site.test/index.md"),
            committed("https://site.test/a", "run-b/site.test/a.md"),
        ]);
        assert_eq!(recorded_output_root(&store), None);
    }

    /// No committed record, or a committed record without a location: fail
    /// OPEN rather than refuse a legitimate resume.
    #[test]
    fn recorded_output_root_fails_open_without_evidence() {
        assert_eq!(recorded_output_root(&DomainRecords::new()), None);

        let mut without_location = committed("https://site.test/a", "ignored");
        without_location.output_location = None;
        assert_eq!(recorded_output_root(&records(vec![without_location])), None);
    }

    /// The recorded root is accepted when it is either directory the run would
    /// write into: the persistence root, or the RAG export dir they diverge to
    /// under `--quick-save` / `--vault`.
    #[test]
    fn guard_accepts_both_directories_this_run_would_write_into() {
        let store = records(vec![committed(
            "https://site.test/a",
            "run-a/site.test/a.md",
        )]);
        let recorded = std::path::Path::new("run-a/site.test");
        assert!(guard_recorded_output_root(&store, recorded, &normalize(recorded)).is_ok());
        assert!(
            guard_recorded_output_root(&store, std::path::Path::new("docs"), recorded).is_ok(),
            "the export directory alone is enough: the record names where the page went, \
             not which sink wrote it"
        );
    }

    /// The contract change: a resume into any other directory is a usage error
    /// (exit 64) whose message names BOTH directories and the remedy.
    #[test]
    fn guard_refuses_a_resume_into_a_different_output() {
        let store = records(vec![committed(
            "https://site.test/a",
            "run-a/site.test/a.md",
        )]);
        let err = guard_recorded_output_root(
            &store,
            std::path::Path::new("run-b"),
            std::path::Path::new("run-b"),
        )
        .expect_err("a resume into a different output must be refused");
        match err {
            CliExit::UsageError(message) => {
                assert!(
                    message.contains("run-a/site.test"),
                    "the error must name the recorded directory: {message}"
                );
                assert!(
                    message.contains("run-b"),
                    "the error must name the directory this run was pointed at: {message}"
                );
                assert!(
                    message.contains("Resume into the recorded output, or start a fresh state"),
                    "the error must state the remedy: {message}"
                );
            },
            other => panic!("expected UsageError (exit 64), got {other:?}"),
        }
    }

    /// No committed evidence means no refusal, even when the directories
    /// differ — the fail-open half of the contract.
    #[test]
    fn guard_fails_open_without_a_recorded_output() {
        assert!(guard_recorded_output_root(
            &DomainRecords::new(),
            std::path::Path::new("run-b"),
            std::path::Path::new("run-b"),
        )
        .is_ok());
    }
}
