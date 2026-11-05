//! ADR-0004 step-5 announcement machinery: which notice this run owes the
//! operator, and the notice text itself.
//!
//! Extracted from `ai.rs` to bring it under the ~500-line file-size rule.
//! The public surface (`ORPHAN_THRESHOLD_NOTICE_MARKER`,
//! `orphan_threshold_notice`, `PendingNotices`) stays byte-stable through
//! the `pub use` re-export in `ai.rs`; `pending_notices` widens to
//! `pub(super)` because `AiArgs::from_matches` is its caller. The
//! `deprecation_notice_tests` module travels with the text it pins.

use super::chosen_by_operator;

/// Distinctive substring of [`orphan_threshold_notice`], used as the count
/// anchor by the behavioral tests.
///
/// Counted, not merely searched, so "announced exactly once per run" is a real
/// assertion. It is also a guard on THIS module: if the anchor appeared twice
/// inside the notice itself, every count would report 2 and read as a
/// regression that does not exist.
pub const ORPHAN_THRESHOLD_NOTICE_MARKER: &str = "calibrado para Granite";

/// ADR-0004 step 5: the orphaned default relevance threshold (spec T5 / S4).
///
/// The shipped [`DEFAULT_THRESHOLD`](crate::domain::options_spec::ai::DEFAULT_THRESHOLD)
/// was calibrated for Granite, and Granite is the local ONNX model this ADR
/// removes: no model left in the box is calibrated for it, so a remote
/// endpoint retains differently under the same number. ADR-0004 makes the
/// mitigation mandatory with an exit criterion — warn while the threshold is
/// still the default, capped at 2 minor releases.
///
/// `threshold` is interpolated rather than restated as a literal so the warning
/// cannot describe a number the run is not using.
#[must_use]
pub fn orphan_threshold_notice(threshold: f32) -> String {
    format!(
        concat!(
            "El umbral de relevancia por defecto ({threshold}) está calibrado para Granite ",
            "(ONNX local). Sin modelo local, un endpoint remoto retiene distinto con el ",
            "mismo número: elegí `--threshold` (o `WEBFANG_THRESHOLD`) según tu modelo. ",
            "Referencia: ADR-0004, sección «Umbral por defecto huérfano»."
        ),
        threshold = threshold
    )
}

/// Which ADR-0004 step-5 announcement THIS run owes the operator.
///
/// Produced at parse time by `AiArgs::from_matches` and consumed by
/// `webfang_cli::main` AFTER `init_logging_dual`. Both halves are forced:
///
/// - **The decision cannot be made at the emit site.** "Did the operator
///   choose `--threshold`" is a `value_source` distinction that does not
///   survive into `AiConfig` — the value alone cannot tell an explicit `0.3`
///   from the default. Only parse time knows.
/// - **The emission cannot happen at parse time.** argv normalization runs
///   before the tracing subscriber exists (#796), so a `warn!` raised there is
///   silently dropped.
///
/// `webfang_cli` therefore owns the `warn!` event, and this type is only the
/// verdict. It is `Copy` and carries no allocation: the notice itself is const
/// text rendered on demand at emit time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PendingNotices {
    /// The threshold is still the default, so emit
    /// [`orphan_threshold_notice`] with this value. `None` when the operator
    /// chose `--threshold` / `WEBFANG_THRESHOLD` themselves.
    pub orphan_threshold: Option<f32>,
}

/// Did this run actually TOUCH the AI surface?
///
/// ONE door: `--clean-ai`. The other doors of the ADR-0004 announcement — the
/// model-selection flag and its env vars — are gone with the shims they
/// announced (issue #2119), so a run that asks for semantic
/// cleaning is the only run with a relevance threshold worth qualifying.
/// `--clean-ai` comes from the crawler group rather than this one, but
/// `ArgMatches` is the whole command's matches, so the id resolves here.
///
/// # Why the notice is CONDITIONAL and not on every run (decided, do not
/// re-open)
///
/// A warning printed on every invocation is a warning nobody reads: it puts
/// the same bytes in front of an operator scraping a single page with no AI
/// involvement at all, and the trained response to a constant stderr line is to
/// stop reading stderr — which would swallow the run that genuinely needs the
/// notice.
///
/// The discipline is deliberately the same one the retired shim warning
/// followed: it speaks only when the thing it qualifies is in play. The
/// behavioral test `orphan_threshold_warning_does_not_fire_with_an_explicit_threshold`
/// pins the other direction.
fn touches_ai_surface(m: &clap::ArgMatches) -> bool {
    chosen_by_operator(m, "clean_ai")
}

/// Decide which ADR-0004 step-5 announcement this run owes (spec T5).
///
/// Returns the verdict; it emits nothing. `webfang_cli::main` renders it after
/// `init_logging_dual` — see [`PendingNotices`] for why both halves must live
/// where they do.
///
/// Called from `AiArgs::from_matches`, the single reader every build goes
/// through, so the notice is feature-independent by construction.
pub(super) fn pending_notices(
    m: &clap::ArgMatches,
    threshold: f32,
    threshold_explicit: bool,
) -> PendingNotices {
    if !touches_ai_surface(m) {
        return PendingNotices::default();
    }
    // The orphan-threshold mitigation's precondition is precisely "the
    // threshold is still the default AND no model is calibrated". Granite was
    // the only calibrated model and it is what ADR-0004 removes, so the
    // default value alone carries the "uncalibrated" half. An operator who
    // typed a threshold has taken the calibration into their own hands:
    // warning them would be noise about a decision they already made.
    PendingNotices {
        orphan_threshold: if threshold_explicit {
            None
        } else {
            Some(threshold)
        },
    }
}

#[cfg(test)]
mod deprecation_notice_tests {
    //! The notice text and the counting anchor the behavioral tests use.
    //!
    //! [`orphan_threshold_notice`]'s interpolation is otherwise UNOBSERVABLE
    //! from the CLI: the warning only fires when the threshold is the default,
    //! so no reachable run can ever show it quoting anything but `0.3`. A
    //! literal pasted into the message would pass every behavioral test and
    //! still be wrong the moment the default moves — this is its only witness.

    use super::{orphan_threshold_notice, ORPHAN_THRESHOLD_NOTICE_MARKER};

    /// The behavioral test asserts a count of exactly 1. That is only
    /// meaningful if the anchor appears once INSIDE the text; an anchor
    /// repeated in the prose would make every count report 2 and read as a
    /// regression that does not exist.
    #[test]
    fn the_anchor_occurs_once_inside_the_orphan_notice_text() {
        assert_eq!(
            orphan_threshold_notice(0.3)
                .matches(ORPHAN_THRESHOLD_NOTICE_MARKER)
                .count(),
            1,
            "the orphan-threshold notice must carry its own counting anchor \
             exactly once"
        );
    }

    /// The message quotes the value it was HANDED, so it can never describe a
    /// threshold the run is not using.
    #[test]
    fn the_orphan_notice_quotes_the_threshold_it_was_given() {
        assert!(
            orphan_threshold_notice(0.55).contains("(0.55)"),
            "the notice must interpolate the value, never a hardcoded literal"
        );
        assert!(
            orphan_threshold_notice(crate::domain::options_spec::ai::DEFAULT_THRESHOLD)
                .contains("(0.3)"),
            "the shipped default must render as `0.3`, not as an f32 artifact"
        );
    }
}
