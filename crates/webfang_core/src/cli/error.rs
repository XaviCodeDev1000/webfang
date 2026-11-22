//! CLI Error Types and Exit Codes
//!
//! T-050: CliError enum with thiserror
//! T-051: CliExit enum with Termination trait for sysexits codes

use std::io::IsTerminal as _;
use std::process::ExitCode;
use thiserror::Error;
use url::Url;

use crate::error::{ErrorClass, ScraperError};

// ============================================================================
// Exit code constants
// ============================================================================

/// Exit 0 — Operation completed successfully.
pub const EXIT_SUCCESS: u8 = 0;
/// Exit 2 — Technical success, no URLs found (empty discovery).
pub const EXIT_EMPTY_DISCOVERY: u8 = 2;
/// Exit 3 — All scrapers failed on discovered URLs.
pub const EXIT_SCRAPER_FAILURE: u8 = 3;
/// Exit 64 — Bad CLI arguments (sysexits EX_USAGE).
pub const EXIT_USAGE_ERROR: u8 = 64;
/// Exit 65 — Data format error (sysexits EX_DATAERR).
pub const EXIT_DATA_ERROR: u8 = 65;
/// Exit 69 — Infrastructure/network failure (sysexits EX_UNAVAILABLE).
pub const EXIT_UNAVAILABLE: u8 = 69;
/// Exit 74 — File I/O error (sysexits EX_IOERR).
pub const EXIT_IO_ERROR: u8 = 74;
/// Exit 76 — Protocol error (sysexits EX_PROTOCOL).
pub const EXIT_PROTOCOL: u8 = 76;
/// Exit 77 — All URLs blocked by robots.txt (sysexits EX_NOPERM).
pub const EXIT_FORBIDDEN: u8 = 77;
/// Exit 78 — Configuration error (sysexits EX_CONFIG).
pub const EXIT_CONFIG: u8 = 78;

// ============================================================================
// T-050: CliError enum
// ============================================================================

/// Categorized CLI errors with user-friendly suggestions.
#[derive(Error, Debug)]
pub enum CliError {
    /// Configuration file error (invalid TOML, missing fields, etc.)
    #[error("Configuración: {msg}\n  Sugerencia: {suggestion}")]
    ConfigFile {
        /// Error message describing the issue
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },

    /// Network-related error (DNS, connection, timeout)
    #[error("Red: {msg}\n  Sugerencia: {suggestion}")]
    NetworkError {
        /// Error message describing the network failure
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },

    /// Some URLs succeeded, others failed
    #[error("Éxito parcial: {success} exitosos, {failed} fallidos\n  Sugerencia: {suggestion}")]
    PartialSuccess {
        /// Number of successfully scraped URLs
        success: u32,
        /// Number of failed URLs
        failed: u32,
        /// Suggested action
        suggestion: String,
    },

    /// Pre-flight validation failed (e.g., unreachable seed URL)
    #[error("Verificación previa fallida: {msg}\n  Sugerencia: {suggestion}")]
    PreflightFailed {
        /// Error message describing the failure
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },
}

impl CliError {
    /// Get the human-readable category name for this error.
    pub fn category(&self) -> &'static str {
        match self {
            CliError::ConfigFile { .. } => "Configuración",
            CliError::NetworkError { .. } => "Red",
            CliError::PartialSuccess { .. } => "Éxito parcial",
            CliError::PreflightFailed { .. } => "Verificación previa",
        }
    }

    /// Get the suggestion text for this error.
    pub fn suggestion(&self) -> &str {
        match self {
            CliError::ConfigFile { suggestion, .. } => suggestion,
            CliError::NetworkError { suggestion, .. } => suggestion,
            CliError::PartialSuccess { suggestion, .. } => suggestion,
            CliError::PreflightFailed { suggestion, .. } => suggestion,
        }
    }
}

/// Format a CliError for display, respecting the color decision (XP-K-04).
///
/// The emoji prefix (ANSI-class decoration) is only used when stderr is an
/// interactive terminal AND the caller has not requested no-color; redirected
/// output gets the plain `[ERROR]` prefix.
pub fn format_cli_error(err: &CliError, no_color: bool) -> String {
    let ansi = crate::domain::console::ansi_enabled(std::io::stderr().is_terminal(), no_color);
    let prefix = if ansi { "❌" } else { "[ERROR]" };
    let category = err.category();
    let msg = match err {
        CliError::ConfigFile { msg, .. } => msg,
        CliError::NetworkError { msg, .. } => msg,
        CliError::PartialSuccess {
            success, failed, ..
        } => &format!("{success} exitosos, {failed} fallidos"),
        CliError::PreflightFailed { msg, .. } => msg,
    };
    let suggestion = err.suggestion();

    format!("{prefix} {category}\n  {msg}\n  Sugerencia: {suggestion}")
}

// ============================================================================
// T-051: CliExit enum with Termination trait
// ============================================================================

/// Exit codes following sysexits convention:
/// 0 = success, 64 = usage error, 65 = data format error, 69 = service unavailable (network/partial),
/// 74 = I/O error, 76 = protocol error, 77 = forbidden (robots.txt), 78 = config error
#[derive(Debug, Clone, PartialEq)]
pub enum CliExit {
    /// Exit 0 — everything OK
    Success,
    /// Exit 64 — bad usage / input
    UsageError(String),
    /// Exit 65 — data format error (malformed XML, etc.)
    DataFormatError(String),
    /// Exit 69 — network / service unavailable
    NetworkError(String),
    /// Exit 74 — I/O error
    IoError(String),
    /// Exit 76 — protocol error
    ProtocolError(String),
    /// Exit 77 — all URLs blocked by robots.txt
    Forbidden(String),
    /// Exit 78 — configuration error
    ConfigError(String),
    /// Exit 2 — no URLs discovered from sitemaps (technical success, null result)
    EmptyDiscovery(String),
    /// Exit 3 — all scrapers failed on discovered URLs
    ScraperFailure(String),
    /// Exit 69 — some URLs succeeded, some failed
    PartialSuccess {
        /// Number of successfully scraped URLs
        success: usize,
        /// Number of failed URLs
        failed: usize,
    },
}

// ============================================================================
// Error Classification Matrix — class → exit mapping (CLI boundary only)
//
// Contract: docs/error-classification-matrix.md (ID 261bdb66-197e-420f-a73b-
// 66c0e889102d), sections "Default exit codes by class", "Typed overrides over
// class defaults", and "Special cell — Cancelled". Exit-code knowledge stays
// OUT of the domain: classification lives in `CrawlError::classify()`, this is
// the only place that turns a class into an exit code.
// ============================================================================

/// Default CLI exit code for an [`ErrorClass`], per the matrix row
/// "Default exit codes by class".
///
/// Returns `None` for [`ErrorClass::PermanentFatal`] and
/// [`ErrorClass::DomainRecoverable`] because neither has a single default:
/// `PermanentFatal` is variant-dependent (64/65/69/76/77/78 per contract rows)
/// and `DomainRecoverable` depends on run outcome (0 if any item succeeded,
/// 65 if ALL items failed). Forcing either onto one code would misreport.
#[must_use]
pub const fn default_exit_code_for_class(class: ErrorClass) -> Option<u8> {
    match class {
        ErrorClass::TransientRetriable | ErrorClass::TransientBackoff => Some(EXIT_UNAVAILABLE),
        // An internal bug is a job failure, NEVER UsageError 64.
        ErrorClass::InternalFatal => Some(EXIT_SCRAPER_FAILURE),
        ErrorClass::PermanentFatal | ErrorClass::DomainRecoverable => None,
    }
}

/// Build the default [`CliExit`] for an [`ErrorClass`] with a caller-supplied
/// message. See [`default_exit_code_for_class`] for why the two
/// variant-dependent classes return `None`.
#[must_use]
pub fn cli_exit_for_class(class: ErrorClass, message: impl Into<String>) -> Option<CliExit> {
    match class {
        ErrorClass::TransientRetriable | ErrorClass::TransientBackoff => {
            Some(CliExit::NetworkError(message.into()))
        },
        ErrorClass::InternalFatal => Some(CliExit::ScraperFailure(message.into())),
        ErrorClass::PermanentFatal | ErrorClass::DomainRecoverable => None,
    }
}

/// Typed override — all URLs blocked by robots.txt / WAF → [`CliExit::Forbidden`]
/// (exit 77). Caller lacks permission; that is not a service fault.
///
/// Fires only when NOTHING was scraped and NOTHING failed, mirroring the
/// routing guard in `orchestrator::report_phase`; this function is the
/// canonical form so the orchestrator can adopt it without behavior change.
#[must_use]
pub fn forbidden_exit_when_all_blocked(
    results_len: usize,
    failures_len: usize,
    blocked: usize,
) -> Option<CliExit> {
    (results_len == 0 && failures_len == 0 && blocked > 0).then(|| {
        CliExit::Forbidden(format!(
            "{blocked} URL(s) bloqueadas por robots.txt. Usa --ignore-robots para omitir esta verificación."
        ))
    })
}

/// Typed override — extraction failures → [`CliExit::DataFormatError`] (exit 65).
/// Content-quality failure, not network: pages were fetched but carried no
/// usable content.
///
/// Canonical form of the typed `matches!` routing in
/// `orchestrator::data_format_error_for_extraction_failed`. Returns `None`
/// when no failure is an extraction failure.
#[must_use]
pub fn data_format_error_exit_when_extraction_failed(
    failures: &[(String, ScraperError)],
) -> Option<CliExit> {
    let extraction_failed = failures
        .iter()
        .filter(|(_, e)| matches!(e, ScraperError::ExtractionFailed { .. }))
        .count();
    (extraction_failed > 0).then(|| {
        CliExit::DataFormatError(format!(
            "extracción sin contenido útil: {extraction_failed} URL(s) devolvieron contenido insuficiente o requieren renderizado de JavaScript"
        ))
    })
}

/// Typed override — internal-fatal failures → [`CliExit::ScraperFailure`]
/// (exit 3, matrix row "Default exit codes by class" → `InternalFatal`).
/// An internal bug is a job failure, never a transient network outage.
///
/// Canonical form of the severity routing in
/// `orchestrator::report_phase` / `orchestrator::batch_exit_code`.
/// Returns `None` when no failure classifies as
/// [`ErrorClass::InternalFatal`], letting the caller keep its fallback arm.
#[must_use]
pub fn scraper_failure_exit_when_internal_fatal(
    failures: &[(String, ScraperError)],
) -> Option<CliExit> {
    let internal_fatal = failures
        .iter()
        .filter(|(_, e)| e.classify() == ErrorClass::InternalFatal)
        .count();
    (internal_fatal > 0).then(|| {
        CliExit::ScraperFailure(format!(
            "Scraper failure: {internal_fatal} internal error(s) out of {} URLs",
            failures.len()
        ))
    })
}

/// Typed override — sitemap empty / sitemap not found →
/// [`CliExit::EmptyDiscovery`] (exit 2). Technical success with a null result,
/// not an operational failure.
#[must_use]
pub fn empty_discovery_exit_for(error: &ScraperError) -> Option<CliExit> {
    matches!(
        error,
        ScraperError::SitemapEmpty | ScraperError::SitemapNotFound(_)
    )
    .then(|| CliExit::EmptyDiscovery(format!("no URLs discovered: {error}")))
}

/// Typed override — a `--dry-run` whose seed the SSRF entry guard refuses →
/// [`CliExit::EmptyDiscovery`] (exit 2).
///
/// The guard cuts a forbidden literal **before any socket opens**, so discovery
/// completes `Ok` with zero URLs and the preview used to print
/// `Dry-run: 0 URL(s) would be scraped:` and exit 0 — a false clean for
/// automation, indistinguishable from a site that genuinely carries no links
/// (#1381). Exit 2 is the same code the sitemap null-result arms already
/// return through [`empty_discovery_exit_for`]: a null result, not a transient
/// network failure, so it is deliberately NOT 69.
///
/// Only the literal-entry layer is named here. A hostname whose answer set is
/// refused at dial time by the validating resolver stays `None`: to the CLI its
/// error is indistinguishable from a real DNS failure, and claiming an SSRF
/// refusal it cannot prove would trade one misleading message for another. That
/// residue is documented in `docs/ssrf-layers.md`.
#[must_use]
pub fn empty_discovery_exit_when_seed_refused(seed: &Url) -> Option<CliExit> {
    crate::domain::ssrf_guard::seed_guard_refusal(seed).map(|refusal| {
            CliExit::EmptyDiscovery(format!(
                "{refusal}. La vista previa no descubrió URLs porque el guard cortó la semilla antes \
                 de abrir conexión. Para un destino interno de confianza: \
                 {entry}=1 (IP literal) y {resolver}=1 (nombre que resuelve a una dirección interna). \
                 Capas y variables: docs/ssrf-layers.md",
                entry = crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                resolver = crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
            ))
        })
}

/// Typed override — config errors ([`ScraperError::Config`] /
/// [`ScraperError::H2Config`]) → [`CliExit::ConfigError`] (exit 78, EX_CONFIG).
#[must_use]
pub fn config_error_exit_for(error: &ScraperError) -> Option<CliExit> {
    matches!(error, ScraperError::Config(_) | ScraperError::H2Config(_))
        .then(|| CliExit::ConfigError(error.to_string()))
}

/// Typed override — permanent I/O errors → [`CliExit::IoError`] (exit 74,
/// EX_IOERR).
///
/// Transient I/O kinds (`Interrupted`, `WouldBlock`, `TimedOut`) return `None`:
/// they classify [`ErrorClass::TransientRetriable`] and keep the class default
/// (exit 69) instead of the permanent-Io override.
#[must_use]
pub fn permanent_io_error_exit_for(error: &std::io::Error) -> Option<CliExit> {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
    ) {
        None
    } else {
        Some(CliExit::IoError(error.to_string()))
    }
}

/// Canonical exit for an elastic-ingestion failure (matrix row 26, #1949).
///
/// The batch-flow call site routes EVERY ingestion failure through this
/// helper — zero inline `CliExit` constructions — so the exit code is decided
/// by the classification machinery instead of the hardcoded `IoError` (74)
/// that used to tell the operator "I/O problem" for what the matrix declares
/// "backend unavailable, retry" (69).
///
/// Precedence, mirroring the all-failed chains (`report_phase` /
/// `batch_exit_code`):
///
/// 1. Typed overrides that can fire from the ingestion pipeline:
///    [`config_error_exit_for`] → 78 (the ingestion downloader's SSRF entry
///    refusal is a `Config` error); `permanent_io_error_exit_for` → 74 (a
///    sink write failure with a permanent io kind).
/// 2. Class default via [`cli_exit_for_class`]: the transient backend failure
///    (network-class re-download error, timeout) → [`CliExit::NetworkError`]
///    (69, EX_UNAVAILABLE — matrix row 26); [`ErrorClass::InternalFatal`] →
///    [`CliExit::ScraperFailure`] (3) — a panicked or cancelled ingestion task
///    (`ScraperError::ingestion`, constructed by `join_failure` in
///    `cli/elastic.rs`), a closed CPU bridge or a repository persistence
///    failure is a data-loss defect, never a retryable outage.
/// 3. Terminal arm for the classes [`default_exit_code_for_class`]
///    deliberately leaves unpinned: for this call site the reachable member
///    is `PayloadTooLarge` (matrix row 14 → 65); the remaining
///    variant-dependent classes have no ingestion-reachable variant today.
#[must_use]
pub fn ingestion_exit_for(error: &ScraperError) -> CliExit {
    let message = format!("Falló la ingesta de vectores: {error}");
    if let Some(exit) = config_error_exit_for(error) {
        return exit;
    }
    if let ScraperError::Io(io_err) = error {
        if let Some(exit) = permanent_io_error_exit_for(io_err) {
            return exit;
        }
    }
    if let Some(exit) = cli_exit_for_class(error.classify(), message.clone()) {
        return exit;
    }
    CliExit::DataFormatError(message)
}

/// Special cell — Cancelled. Cooperative cancellation is a control signal, NOT
/// an operational failure: intercepted BEFORE any classification-based routing,
/// it yields [`CliExit::Success`] (exit 0).
///
/// The domain's defensive fallback (`CrawlError::Cancelled → InternalFatal`)
/// only covers signals escaping through unexpected paths; this function is the
/// primary interception point at the CLI boundary.
#[must_use]
pub fn cancelled_exit(cancelled: bool) -> Option<CliExit> {
    cancelled.then_some(CliExit::Success)
}

impl std::process::Termination for CliExit {
    fn report(self) -> ExitCode {
        match self {
            CliExit::Success => ExitCode::from(EXIT_SUCCESS),
            CliExit::UsageError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_USAGE_ERROR)
            },
            CliExit::DataFormatError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_DATA_ERROR)
            },
            CliExit::NetworkError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_UNAVAILABLE)
            },
            CliExit::IoError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_IO_ERROR)
            },
            CliExit::ProtocolError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_PROTOCOL)
            },
            CliExit::Forbidden(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_FORBIDDEN)
            },
            CliExit::ConfigError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_CONFIG)
            },
            CliExit::EmptyDiscovery(msg) => {
                eprintln!("Warning: {msg}");
                ExitCode::from(EXIT_EMPTY_DISCOVERY)
            },
            CliExit::ScraperFailure(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_SCRAPER_FAILURE)
            },
            CliExit::PartialSuccess { .. } => ExitCode::from(EXIT_UNAVAILABLE),
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "error_matrix_tests.rs"]
mod matrix_tests;

#[cfg(test)]
#[path = "error_overrides_tests.rs"]
mod overrides_tests;
