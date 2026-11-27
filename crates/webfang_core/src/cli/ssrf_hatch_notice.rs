//! The one human-facing notice for the SSRF entry hatch (T1A, issue #3080).
//!
//! [`DISABLE_ENTRY_GUARD_ENV`] is the only SSRF hatch a SHIPPED binary honours (its siblings are
//! `cfg(test)`-confined, S9), and before this module it armed itself in total
//! silence: an operator who exported it got a wider-looking hole than they
//! actually opened. The name suggests the whole guard; the effect is the
//! literal-IP rejection at the ENTRY boundary alone — the connect-time
//! validating DNS resolver and the redirect policy stay armed on every hop.
//!
//! # The announcement boundary (contract, not an accident)
//!
//! **Every run that reaches the scrape pipeline announces the hatch.**
//! [`emit_entry_hatch_notice`] is the first statement of
//! [`orchestrator::run`](crate::cli::orchestrator::run), and that function is
//! the single entry of every route the CLI can take through the scraper — dry
//! run, batch, and crawl/scrape. `run` runs once per process, so exactly one
//! line reaches stderr per run.
//!
//! **Every run that never reaches the scraper announces nothing, and that
//! silence is intended.** This hatch configures the SCRAPER's literal-IP entry
//! guard: a path that exits before any scrape — a clap usage error, the config
//! preflight, `doctor`'s provider probes, `connect` / `show` / `recover` /
//! `completions` — never consults that guard, so there is no scraper behaviour
//! to qualify and nothing to warn about. A notice there would describe a
//! decision the run never takes.
//!
//! The second reason is structural and rules out any earlier site:
//! `webfang_cli::main` installs its ONLY tracing subscriber after subcommand
//! dispatch and after the config preflight, so those paths hold no subscriber
//! at all and could not render a traced notice even if one were owed; a
//! `warn!` raised at the process entry is dropped by the not-yet-installed
//! subscriber (#796). The orchestrator entry is the earliest point that is
//! simultaneously downstream of the subscriber and part of a run, which is
//! exactly why the call site lives there rather than at the arming site.
//!
//! Both halves are pinned: `hatch_armed_warns_exactly_once_with_its_exact_scope`
//! and `hatch_absent_emits_no_such_notice` for the announcing side,
//! `a_clap_usage_error_never_announces_the_hatch` for the silent side.

use crate::domain::ssrf_guard::{entry_guard_disabled_by_env, DISABLE_ENTRY_GUARD_ENV};

/// The phrase the notice exists to say, exported so the tests assert on the
/// same source of truth the binary prints instead of on a copy of it.
///
/// Deliberately a whole clause rather than a word: a caller counting
/// occurrences in captured stderr gets the number of NOTICES, never the number
/// of lines that happen to mention the entry guard.
pub const SSRF_ENTRY_HATCH_NOTICE_MARKER: &str = "only the literal-IP entry rejection is disabled";

/// Announce an armed entry hatch, once per run, naming its exact scope.
///
/// Called exactly once per run, as the first statement of
/// [`orchestrator::run`](crate::cli::orchestrator::run) — the only entry every
/// scraper route walks. Silent when the hatch is absent or carries any value
/// other than `"1"` ([`entry_guard_disabled_by_env`]).
///
/// Runs that exit before the scraper (clap usage errors, the config preflight,
/// `doctor`, `connect`, `show`, `recover`, `completions`) never call this and
/// must not: they never consult the entry guard, and they hold no tracing
/// subscriber to render an event through. See the module documentation for the
/// full boundary contract.
///
/// The prose travels once, as the event message; the structured fields carry
/// only what a log query needs, so the compact formatter never prints the same
/// sentence twice (the same shape the orphan-threshold notice follows).
///
/// MUST be called after the tracing subscriber is installed: called earlier,
/// the event is dropped and the hatch stays silent, which is the very defect
/// this function exists to remove.
pub fn emit_entry_hatch_notice() {
    if !entry_guard_disabled_by_env() {
        return;
    }
    // `target: "webfang"` pins the module path operators and snapshots key on:
    // this is the CLI's own notice, not an infrastructure log line, and moving
    // the function between child modules must not rename its source.
    tracing::warn!(
        target: "webfang",
        notice = "ssrf_entry_hatch_armed",
        disabled_layer = "literal-IP entry rejection",
        still_enforced = "validating DNS resolver + redirect policy",
        "{} = 1: only the literal-IP entry rejection is disabled; the validating \
         DNS resolver and the redirect policy stay enforced on every hop. The \
         hatch remains available in release builds — unset it to restore the \
         full guard.",
        DISABLE_ENTRY_GUARD_ENV,
    );
}
