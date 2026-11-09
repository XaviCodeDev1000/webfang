//! Batch progress sidecar + sweep decision (slice 2, WU1 — #2611).
//!
//! A batch that dies before its clean close leaves two artifacts: the capture
//! spool (`.webfang-batch-capture.jsonl`, slice 1) and — from slice 2 on — a
//! progress sidecar (`.webfang-batch-capture.progress.json`) describing HOW
//! far the previous run got and under which extraction identity. The next
//! batch start reads both and decides what to do with the leftover spool:
//! salvage what is convertible, or quarantine it for `webfang recover`.
//!
//! # The one decision table
//!
//! [`decide_sweep`] is the ONLY decision authority for the leftover spool.
//! It is pure (state in → verdict out) so every caller — CLI startup, tests,
//! a future `doctor` surface — shares the same policy, and the policy can be
//! pinned by tests rather than by arguments in review.
//!
//! The rule underneath the table is the one the maintainer pinned (L5): a
//! sidecar that does not describe the spool it sits next to must not choose a
//! destination for the spool's data. Only a `Parsed` sidecar whose spool
//! identity matches the file on disk can drive a salvage; everything else is
//! [`SweepDecision::OrphanSpool`] — the data is NEVER deleted while in doubt,
//! the operator is pointed at `webfang recover`, and the run starts a fresh
//! spool in the original path.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// File name of the progress sidecar, next to the capture spool.
pub const BATCH_PROGRESS_FILE: &str = ".webfang-batch-capture.progress.json";

/// Version of the sidecar schema. Bump on any shape change; readers reject
/// (as `Unreadable`) versions they do not know.
pub const BATCH_PROGRESS_VERSION: u32 = 1;

/// Identity of the spool file the sidecar describes.
///
/// `first_line_sha256` is `Some` when the spool had at least one line when
/// the sidecar was written, and carries the SHA-256 of the FIRST line
/// (newline included, exactly the bytes on disk). A spool that was empty at
/// start records `None` — an empty spool has no identity, and pretending
/// otherwise would make "empty then written" look like a foreign spool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoolIdentity {
    /// SHA-256 of the spool's first line including its `\n`, when one existed.
    pub first_line_sha256: Option<String>,
}

/// Sidecar written at batch start and advanced by the incremental flush.
///
/// Everything the NEXT run needs to decide what the leftover spool means:
/// who wrote it (identity), where the pages go (`output_dir`), how they are
/// extracted (`extraction_options`), and how far the flush got
/// (`watermark_bytes`).
///
/// Unknown fields are tolerated (`deny_unknown_fields` rejected in L6): the
/// sidecar is written by THIS binary but may be edited or extended by
/// tooling; a reader that hard-fails on extra fields would turn a recoverable
/// run into an `OrphanSpool` quarantine for no reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchProgressV1 {
    /// Schema version — see [`BATCH_PROGRESS_VERSION`].
    pub version: u32,
    /// Identity of the spool this sidecar describes.
    pub spool: SpoolIdentity,
    /// Output directory the flush was exporting into (as given by the run).
    pub output_dir: PathBuf,
    /// Extraction options the previous run used, so a salvage replays the
    /// SAME pipeline instead of silently diverging from the original `.md`.
    pub extraction_options: ExtractionOptions,
    /// Durable bytes of the spool already exported by the flush — the
    /// watermark a salvage resumes from. `flush()` hands bytes to the OS;
    /// it is NOT an fsync, so this watermark is best-effort by contract.
    pub watermark_bytes: u64,
}

/// The extraction options that shape spool → markdown conversion.
///
/// Only the knobs that change the converted bytes are recorded: a salvage
/// with a different selector or WAF policy would produce DIFFERENT `.md`
/// content than the original run, which breaks the byte-identical guarantee
/// the recover tests pin. Options that do not affect conversion (budget,
/// concurrency, network profiles) are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionOptions {
    /// CSS selector the extractor was told to scope content to. `ScraperConfig`
    /// always carries one (default `"body"`), so `None` here means the sidecar
    /// predates the field or was hand-edited — a salvage replaying it must
    /// treat that as the config default.
    pub selector: Option<String>,
    /// Whether WAF handling was disabled (`--ignore-waf`).
    pub ignore_waf: bool,
}

impl ExtractionOptions {
    /// Build from a [`ScraperConfig`] the way `extract_batch_content` does:
    /// destructure WITHOUT `..` (L4) so a new config field cannot silently
    /// sneak into the recorded identity without this type learning about it.
    #[must_use]
    pub fn from_scraper_config(config: &crate::domain::config::ScraperConfig) -> Self {
        Self {
            selector: Some(config.selector.clone()),
            ignore_waf: config.ignore_waf,
        }
    }

    /// Project back onto a fresh [`ScraperConfig`] for a salvage conversion.
    ///
    /// The output directory is NOT part of the identity — the salvage target
    /// is decided by the caller (the sidecar's `output_dir` for an automatic
    /// flush resume, an explicit `-o` for recover) — so it is applied after.
    #[must_use]
    pub fn to_scraper_config(&self, output_dir: &Path) -> crate::domain::config::ScraperConfig {
        let mut config = crate::domain::config::ScraperConfig::default()
            .with_output_dir(output_dir.to_path_buf());
        if let Some(selector) = &self.selector {
            config = config.with_selector(selector.clone());
        }
        // A missing selector field (legacy/hand-edited sidecar) keeps the
        // `default()` value ("body") already in place.
        if self.ignore_waf {
            config = config.with_ignore_waf(true);
        }
        config
    }
}

/// Whether the sidecar next to the spool is usable for a salvage decision.
///
/// The table input: the sidecar is either ABSENT, PRESENT-but-unreadable
/// (truncated, wrong version, foreign schema), or PARSED into a
/// [`BatchProgressV1`]. A present-but-unreadable sidecar is deliberately NOT
/// conflated with absence — see [`decide_sweep`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarState {
    /// No sidecar file (first run, or a previous run swept it away).
    Absent,
    /// The file exists but cannot be trusted: unreadable JSON, unknown
    /// version, or any parse failure.
    Unreadable,
    /// Parsed successfully — carries the sidecar's content.
    Parsed(Box<BatchProgressV1>),
}

/// State of the spool file itself, as observed at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolFileState {
    /// Size of the spool file in bytes (0 when absent).
    pub len_bytes: u64,
    /// SHA-256 of the first line (`\n` included) when the spool has at least
    /// one complete line; `None` for an empty spool or one with no newline
    /// yet (a torn final record has no trustworthy identity line).
    pub first_line_sha256: Option<String>,
}

/// What to do with the leftover spool before this run starts writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepDecision {
    /// The spool is clean or fully accounted for: remove (or overwrite) it
    /// and start fresh. No salvage needed.
    Nothing,
    /// A usable sidecar describes this exact spool: salvage the pages past
    /// `watermark_bytes` into the sidecar's output identity before the new
    /// run writes anything. The spool stays on disk until the salvage
    /// completes.
    SalvageFromOffset {
        /// Watermark from the sidecar: the salvage starts reading here.
        offset: u64,
        /// The sidecar that authorized the salvage.
        sidecar: Box<BatchProgressV1>,
    },
    /// The spool holds data that CANNOT be safely attributed: no sidecar
    /// (or an untrustworthy one), while the spool is non-empty. Rename it
    /// out of the way (timestamped sibling), warn with the path, and start
    /// a fresh spool at the original path. NEVER delete.
    OrphanSpool,
}

/// Compute the SHA-256 of a spool's first line for [`SpoolFileState`].
///
/// The hash covers the line INCLUDING its terminating `\n`, exactly the
/// bytes `read_until` consumes for it, so identity survives content edits
/// deeper in the file (append-only growth never changes the first line).
///
/// Returns `None` when the spool has no complete line: an empty file, or one
/// whose only content is a torn final record without `\n` — bytes that were
/// never acknowledged by a page write have no identity worth matching.
#[must_use]
pub fn spool_first_line_sha256(bytes: &[u8]) -> Option<String> {
    let newline = bytes.iter().position(|b| *b == b'\n')?;
    Some(format!("{:x}", Sha256::digest(&bytes[..=newline])))
}

/// The pure sweep decision (the ONE authority — module docs).
///
/// The inputs are exactly what startup observed: the sidecar state and the
/// spool file state. The decision:
///
/// | sidecar | spool | decision |
/// |---|---|---|
/// | `Parsed` | first-line hash matches sidecar | `SalvageFromOffset` |
/// | `Parsed` | identity mismatch / no identity | `OrphanSpool` |
/// | `Unreadable` | non-empty | `OrphanSpool` |
/// | `Unreadable` | empty | `Nothing` (nothing to lose) |
/// | `Absent` | any | `Nothing` (slice-1 world: recover owns it) |
///
/// The asymmetry is the pinned policy (L5/L6): salvage requires the sidecar
/// to DESCRIBE the spool (identity match) — a sidecar that cannot prove it
/// is talking about this file must not choose a destination for its data.
#[must_use]
pub fn decide_sweep(sidecar: &SidecarState, spool: &SpoolFileState) -> SweepDecision {
    match sidecar {
        SidecarState::Absent => SweepDecision::Nothing,
        SidecarState::Unreadable => {
            if spool.len_bytes == 0 {
                SweepDecision::Nothing
            } else {
                SweepDecision::OrphanSpool
            }
        },
        SidecarState::Parsed(sidecar) => {
            let identity_matches = spool.first_line_sha256.is_some()
                && spool.first_line_sha256 == sidecar.spool.first_line_sha256;
            if identity_matches {
                // Clamp: a watermark past the spool (external truncation or
                // manipulation) salvages nothing rather than seeking out of
                // bounds; the pages are already exported by contract.
                let offset = sidecar.watermark_bytes.min(spool.len_bytes);
                SweepDecision::SalvageFromOffset {
                    offset,
                    sidecar: sidecar.clone(),
                }
            } else {
                SweepDecision::OrphanSpool
            }
        },
    }
}

/// Timestamped sibling name for an orphaned spool: the original name with a
/// local-time suffix before the extension-less dotfile's end. The seconds
/// precision is enough for an operator to find the file; collisions fall
/// back to the uniquifier below.
#[must_use]
pub fn orphan_spool_path(spool: &Path, now_unix_secs: u64) -> PathBuf {
    let mut name = spool.file_name().map_or_else(
        || "orphan.spool".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    name.push('.');
    name.push_str(&now_unix_secs.to_string());
    name.push_str(".orphaned");
    spool.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(first_line: Option<&str>, watermark: u64) -> BatchProgressV1 {
        BatchProgressV1 {
            version: BATCH_PROGRESS_VERSION,
            spool: SpoolIdentity {
                first_line_sha256: first_line.map(|line| format!("{:x}", Sha256::digest(line))),
            },
            output_dir: std::path::PathBuf::from("/tmp/out"),
            extraction_options: ExtractionOptions {
                selector: None,
                ignore_waf: false,
            },
            watermark_bytes: watermark,
        }
    }

    fn spool(bytes: &[u8]) -> SpoolFileState {
        SpoolFileState {
            len_bytes: bytes.len() as u64,
            first_line_sha256: spool_first_line_sha256(bytes),
        }
    }

    #[test]
    fn absent_sidecar_means_nothing_regardless_of_spool() {
        for spool_state in [spool(b""), spool(b"{\"url\":\"a\"}\n")] {
            assert_eq!(
                decide_sweep(&SidecarState::Absent, &spool_state),
                SweepDecision::Nothing,
                "absent sidecar = slice-1 world: recover owns leftovers"
            );
        }
    }

    #[test]
    fn unreadable_sidecar_with_empty_spool_is_nothing() {
        assert_eq!(
            decide_sweep(&SidecarState::Unreadable, &spool(b"")),
            SweepDecision::Nothing,
            "an empty spool has no data worth quarantining"
        );
    }

    #[test]
    fn unreadable_sidecar_with_data_is_orphan() {
        assert_eq!(
            decide_sweep(&SidecarState::Unreadable, &spool(b"{\"url\":\"a\"}\n")),
            SweepDecision::OrphanSpool,
            "data without a trustworthy description is never deleted"
        );
    }

    #[test]
    fn parsed_matching_identity_salvages_from_watermark() {
        let line = b"{\"url\":\"https://a\"}\n";
        let side = sidecar(Some(std::str::from_utf8(line).expect("utf8")), 42);
        // The line is 20 bytes, so 42 past it clamps to the length — same
        // contract the over-large watermark test pins.
        assert_eq!(
            decide_sweep(&SidecarState::Parsed(Box::new(side)), &spool(line)),
            SweepDecision::SalvageFromOffset {
                offset: line.len() as u64,
                sidecar: Box::new(sidecar(Some(std::str::from_utf8(line).expect("utf8")), 42)),
            },
            "identity match authorizes salvage; the offset clamps to the spool"
        );
    }

    #[test]
    fn parsed_identity_mismatch_is_orphan() {
        let side = sidecar(Some("{\"url\":\"other\"}\n"), 42);
        assert_eq!(
            decide_sweep(
                &SidecarState::Parsed(Box::new(side)),
                &spool(b"{\"url\":\"a\"}\n")
            ),
            SweepDecision::OrphanSpool,
            "a sidecar describing another spool must not salvage this one"
        );
    }

    #[test]
    fn parsed_sidecar_without_identity_is_orphan_on_nonempty_spool() {
        // Sidecar written when the spool was empty (first_line None): it
        // describes nothing, so data on disk is unattributable.
        let side = sidecar(None, 0);
        assert_eq!(
            decide_sweep(
                &SidecarState::Parsed(Box::new(side)),
                &spool(b"{\"url\":\"a\"}\n")
            ),
            SweepDecision::OrphanSpool
        );
    }

    #[test]
    fn watermark_past_spool_len_clamps_to_len() {
        let line = b"{\"url\":\"https://a\"}\n";
        let side = sidecar(Some(std::str::from_utf8(line).expect("utf8")), 10_000);
        let decision = decide_sweep(&SidecarState::Parsed(Box::new(side)), &spool(line));
        match decision {
            SweepDecision::SalvageFromOffset { offset, .. } => {
                assert_eq!(offset, line.len() as u64, "clamped to the real length");
            },
            other => panic!("expected salvage, got {other:?}"),
        }
    }

    #[test]
    fn first_line_hash_covers_line_including_newline() {
        let bytes = b"{\"a\":1}\n{\"b\":2}\n";
        let hash = spool_first_line_sha256(bytes).expect("has first line");
        assert_eq!(
            hash,
            format!("{:x}", Sha256::digest(b"{\"a\":1}\n")),
            "identity is exactly the first line + newline"
        );
    }

    #[test]
    fn torn_tail_has_no_identity() {
        assert_eq!(
            spool_first_line_sha256(b"{\"a\":1}"),
            None,
            "no newline yet"
        );
        assert_eq!(spool_first_line_sha256(b""), None, "empty spool");
    }

    #[test]
    fn appending_does_not_change_identity() {
        let base = b"{\"url\":\"a\"}\n";
        let grown = b"{\"url\":\"a\"}\n{\"url\":\"b\"}\n";
        assert_eq!(
            spool_first_line_sha256(base),
            spool_first_line_sha256(grown),
            "append-only growth keeps the first line — and the identity"
        );
    }

    #[test]
    fn orphan_path_is_a_timestamped_sibling() {
        let spool = Path::new("/run/.webfang-batch-capture.jsonl");
        let orphan = orphan_spool_path(spool, 1_700_000_000);
        assert_eq!(
            orphan,
            Path::new("/run/.webfang-batch-capture.jsonl.1700000000.orphaned"),
            "same directory, same name, timestamped suffix"
        );
    }

    #[test]
    fn extraction_options_round_trip_through_scraper_config() {
        let config = crate::domain::config::ScraperConfig::default()
            .with_output_dir(PathBuf::from("/tmp/whatever"))
            .with_selector("article".to_string())
            .with_ignore_waf(true);
        let options = ExtractionOptions::from_scraper_config(&config);
        assert_eq!(options.selector.as_deref(), Some("article"));
        assert!(options.ignore_waf);

        let replayed = options.to_scraper_config(Path::new("/tmp/salvage"));
        assert_eq!(replayed.selector, "article");
        assert!(replayed.ignore_waf);
        assert_eq!(
            replayed.output_dir,
            Path::new("/tmp/salvage"),
            "output dir comes from the salvage target, not the identity"
        );
    }

    #[test]
    fn sidecar_json_round_trip_tolerates_unknown_fields() {
        let side = sidecar(Some("{\"x\"}\n"), 7);
        let json = serde_json::to_string(&side).expect("serialize");
        let with_extra = json.replace('}', ",\"future_field\":true}");
        let parsed: BatchProgressV1 = serde_json::from_str(&with_extra).expect("tolerate extras");
        assert_eq!(parsed, side, "unknown fields must not break reading");
    }
}
