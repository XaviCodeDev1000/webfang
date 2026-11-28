//! Semantic-cleaning degradation marker (T6, issue #3117 / finding E9).
//!
//! `--clean-ai` degrades PER PAGE: a page whose text cannot be chunked or
//! embedded falls back to its raw content, the run keeps going, and — before
//! this module — the only trace of it was a single `WARN` that named one cause
//! for all three variants. A 9-of-10 degradation therefore exited 0 and left an
//! export indistinguishable from a clean one.
//!
//! Three artefacts now carry the same fact, under the same name
//! ([`DEGRADED_REASON_KEY`]):
//!
//! 1. the JSONL line (`degraded_reason` on the exported record),
//! 2. the durable `RecordStore` record,
//! 3. the one tally line a degraded run closes with ([`DegradationTally`]).
//!
//! Additive by construction: the field is an `Option` that is absent when the
//! page cleaned normally, so old exports and old stores keep loading (plan S4).
//!
//! The `SemanticError` → [`DegradationCause`] mapping stays in the CLI layer
//! (`cli::export_flow`): the domain owns the vocabulary and the tally, not the
//! error taxonomy of the cleaner that produced them.

use std::fmt;

/// The single name the degradation fact travels under: the chunk-metadata key
/// the export flow stamps, the JSONL field the exporter writes, and the
/// `RecordStore` field the durable record carries.
///
/// One constant, not three literals, because a consumer that reads one artefact
/// and writes another must not be able to misspell it.
pub const DEGRADED_REASON_KEY: &str = "degraded_reason";

/// Why a page's text fell back to the raw content.
///
/// Exactly the three [`crate::error::SemanticError`] variants that classify as
/// `DomainRecoverable` — the ones a run survives. An infrastructure-fatal
/// failure aborts the run instead of degrading a page, so it is not a cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradationCause {
    /// A single paragraph exceeded the character budget and cannot be split.
    ChunkTooLarge,
    /// The text could not be turned into a chunk at all.
    Tokenize,
    /// A chunker built a byte span with `start > end`.
    InvalidSpan,
}

impl DegradationCause {
    /// Every cause, in the order the tally line reports them.
    ///
    /// Fixed order (not "first seen") so the summary line is deterministic
    /// across runs of the same input.
    pub const ALL: [Self; 3] = [Self::ChunkTooLarge, Self::Tokenize, Self::InvalidSpan];

    /// The variant's own name, as it appears in the artefacts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChunkTooLarge => "ChunkTooLarge",
            Self::Tokenize => "Tokenize",
            Self::InvalidSpan => "InvalidSpan",
        }
    }

    /// This cause's slot in [`DegradationTally`]'s counters.
    const fn index(self) -> usize {
        match self {
            Self::ChunkTooLarge => 0,
            Self::Tokenize => 1,
            Self::InvalidSpan => 2,
        }
    }
}

impl fmt::Display for DegradationCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one cleaning phase cost: how many pages it folded and how many of them
/// degraded, broken down by cause.
///
/// `total` is the number of pages the phase was ASKED to clean (the run's page
/// count), not the number it reached: a cancelled phase reports the same
/// denominator as a completed one, which is what makes "9/10 degraded" mean
/// what a reader expects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DegradationTally {
    total: usize,
    counts: [usize; 3],
}

impl DegradationTally {
    /// An empty tally over `total` pages.
    #[must_use]
    pub const fn new(total: usize) -> Self {
        Self {
            total,
            counts: [0; 3],
        }
    }

    /// Count one degraded page.
    pub fn record(&mut self, cause: DegradationCause) {
        self.counts[cause.index()] += 1;
    }

    /// Pages the phase was asked to clean.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Pages that fell back to their raw content.
    #[must_use]
    pub const fn degraded(&self) -> usize {
        self.counts[0] + self.counts[1] + self.counts[2]
    }

    /// Whether at least one page degraded — the predicate
    /// `--strict-clean-ai` turns into exit 78.
    #[must_use]
    pub const fn is_degraded(&self) -> bool {
        self.degraded() > 0
    }

    /// How many pages degraded with `cause`.
    #[must_use]
    pub const fn count(&self, cause: DegradationCause) -> usize {
        self.counts[cause.index()]
    }
}

impl fmt::Display for DegradationTally {
    /// `Semantic cleaning: 9/10 degraded (ChunkTooLarge: 8, Tokenize: 1)`.
    ///
    /// Zero-count causes are omitted: a breakdown that lists `InvalidSpan: 0`
    /// on every page would bury the one number that matters. With no
    /// degradation at all the parenthesised breakdown is absent, so the line
    /// still states the tally (`Semantic cleaning: 0/10 degraded`) rather than
    /// inventing an empty one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Semantic cleaning: {}/{} degraded",
            self.degraded(),
            self.total
        )?;
        let mut first = true;
        for cause in DegradationCause::ALL {
            let count = self.count(cause);
            if count == 0 {
                continue;
            }
            f.write_str(if first { " (" } else { ", " })?;
            write!(f, "{cause}: {count}")?;
            first = false;
        }
        if !first {
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The two properties the artefacts depend on: the key is the one the
    //! writer and the reader both use, and the rendered line is deterministic.

    use super::{DegradationCause, DegradationTally, DEGRADED_REASON_KEY};

    #[test]
    fn the_marker_key_is_the_documented_field_name() {
        assert_eq!(DEGRADED_REASON_KEY, "degraded_reason");
    }

    #[test]
    fn the_tally_counts_each_cause_and_renders_a_fixed_order() {
        let mut tally = DegradationTally::new(10);
        tally.record(DegradationCause::Tokenize);
        tally.record(DegradationCause::ChunkTooLarge);
        tally.record(DegradationCause::ChunkTooLarge);

        assert_eq!(tally.total(), 10);
        assert_eq!(tally.degraded(), 3);
        assert!(tally.is_degraded());
        assert_eq!(tally.count(DegradationCause::Tokenize), 1);
        assert_eq!(tally.count(DegradationCause::InvalidSpan), 0);
        // Declaration order, not insertion order: the same three pages always
        // render the same bytes.
        assert_eq!(
            tally.to_string(),
            "Semantic cleaning: 3/10 degraded (ChunkTooLarge: 2, Tokenize: 1)"
        );
    }

    #[test]
    fn a_clean_tally_states_the_denominator_and_no_breakdown() {
        let tally = DegradationTally::new(4);
        assert!(!tally.is_degraded());
        assert_eq!(tally.to_string(), "Semantic cleaning: 0/4 degraded");
    }
}
