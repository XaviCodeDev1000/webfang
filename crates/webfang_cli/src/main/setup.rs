//! CLI argument parsing, URL/log-level resolution, the orphan-threshold
//! notice and the file trace layer builder.

use super::*;

/// Render ADR-0004 step 5's orphan-threshold announcement (spec T5) as a
/// `WARN` event, if this run owes it.
///
/// MUST be called after `init_logging_dual`: argv resolution runs before the
/// subscriber exists, so a `warn!` emitted at parse time would be silently
/// dropped (#796). Whether the notice is owed was decided at parse time —
/// `webfang_core::cli::args::ai::PendingNotices` — because the trigger needs a
/// `value_source` distinction that does not survive into `CrawlOptions`.
///
/// Shape: English structured fields carrying the facts, the Spanish prose as
/// the event message. The prose appears EXACTLY once: the compact formatter
/// prints a structured field's value right after the message, so routing the
/// same text through both doubles the line — invisible at ~40 characters,
/// unreadable at the ~600 a deprecation needs. The fields below carry only
/// what is meant to be QUERIED: which notice, what value is in play, what it
/// was calibrated for, what replaces it. Never a second copy of the sentence.
pub(super) fn emit_orphan_threshold_notice(pending: webfang_core::cli::args::ai::PendingNotices) {
    if let Some(threshold) = pending.orphan_threshold {
        tracing::warn!(
            notice = "orphan_relevance_threshold",
            // `display`, not the raw f32: tracing's `Value` impl for `f32`
            // prints the IEEE-754 round-trip form (`0.30000001192092896`), which
            // is a second thing to explain to an operator. The field stays a
            // string in the JSONL — still queryable, and it reads `0.3`.
            threshold_used = tracing::field::display(format_args!("{threshold}")),
            calibrated_for = "granite",
            replacement = "--threshold|WEBFANG_THRESHOLD",
            "{}",
            webfang_core::cli::args::ai::orphan_threshold_notice(threshold),
        );
    }
}

/// Parse CLI arguments, translating clap's help/version/error cases into exits.
///
/// Single parse pass per design D1: `ArgMatches` built once, `Args` derived
/// via `FromArgMatches`, provenance captured via `ArgSources::capture`.
pub(super) fn parse_args() -> Result<(Args, ArgSources), CliExit> {
    use clap::{CommandFactory, FromArgMatches};
    let matches = match Args::command().try_get_matches() {
        Ok(m) => m,
        Err(e) => {
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                e.print().ok();
                return Err(CliExit::Success);
            }
            eprintln!("{e}");
            return Err(CliExit::UsageError("invalid arguments".into()));
        },
    };
    let args = Args::from_arg_matches(&matches).map_err(|e| {
        if e.kind() == clap::error::ErrorKind::DisplayHelp
            || e.kind() == clap::error::ErrorKind::DisplayVersion
        {
            e.print().ok();
            CliExit::Success
        } else {
            eprintln!("{e}");
            CliExit::UsageError("invalid arguments".into())
        }
    })?;
    let sources = ArgSources::capture(&matches);
    Ok((args, sources))
}

/// Ensure a URL is present; without `--url` (and outside batch mode) scraping cannot start.
pub(super) async fn resolve_url(args: &mut Args) -> Result<(), CliExit> {
    // Batch mode reads URLs from stdin/file — --url is not required
    let is_batch = args.export.batch || args.export.batch_file.is_some();

    // If a URL is provided (or batch mode), nothing to resolve.
    if args.crawler.url.is_some() || is_batch {
        return Ok(());
    }

    Err(CliExit::UsageError("--url is required for scraping".into()))
}

/// Map verbosity count to tracing levels (0=WARN, 1=INFO, 2=DEBUG, 3+=TRACE).
pub(super) fn resolve_log_level(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

/// Resolve the effective log level, honoring `log_level` from the config
/// file when the operator gave no `-v` flags (P4-3 of #1296: the key was
/// loaded by `ConfigDefaults` but never consumed — repro posted in the
/// issue). Explicit flags always win; an invalid config value warns on
/// stderr — the tracing subscriber does not exist yet — and falls back to
/// WARN, matching the `ConfigDefaults` graceful-degradation contract.
pub(super) fn resolve_log_level_with_config(
    verbosity: u8,
    config_level: Option<&str>,
) -> &'static str {
    if verbosity > 0 {
        return resolve_log_level(verbosity);
    }
    let normalized = config_level
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    match normalized.as_deref() {
        None | Some("warn") => "warn",
        Some("error") => "error",
        Some("info") => "info",
        Some("debug") => "debug",
        Some("trace") => "trace",
        Some(other) => {
            eprintln!(
                "Advertencia: log_level «{other}» inválido en el archivo de configuración; se usa «warn»."
            );
            "warn"
        },
    }
}

/// Create FileTraceLayer when --trace-file is present (always available, no feature gate).
///
/// The trace file is an explicitly requested artifact: a failure to create it
/// must abort the run (fail closed) instead of silently continuing without the
/// requested trace output (issue #1814).
pub(super) fn build_file_trace_layer(
    trace_file: Option<std::path::PathBuf>,
) -> Result<Option<webfang_core::infrastructure::observability::FileTraceLayer>, CliError> {
    match trace_file {
        Some(path) => {
            webfang_core::infrastructure::observability::FileTraceLayer::new(path.clone())
                .map(Some)
                .map_err(|e| CliError::ConfigFile {
                    msg: format!(
                        "no se pudo crear el archivo de trazas en '{}': {e}",
                        path.display()
                    ),
                    suggestion:
                        "verifique que la ruta padre de --trace-file exista y sea escribible, o elija otra ruta"
                            .to_string(),
                })
        },
        None => Ok(None),
    }
}
