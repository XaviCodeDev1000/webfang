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
use crate::domain::persistence::{PersistenceMode, RecordStoreError, StateStorePort};
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
    crate::domain::paths::cache_base_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("webfang")
        .join("state")
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
/// fresh start that would re-drive committed work.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    mode: &PersistenceMode,
    target_url: &str,
    _root_correlation: &CorrelationId,
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
