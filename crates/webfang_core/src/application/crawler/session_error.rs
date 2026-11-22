//! Error taxonomy for the crawl session run owner.
//!
//! Every failure a `CrawlSession` can raise, with the user-facing wording
//! attached at the definition site. Re-exported from
//! [`super::session`] so consumer paths stay unchanged.

use std::path::PathBuf;

use thiserror::Error;

use crate::domain::error::CrawlError;
use crate::error::ScraperError;

/// Application-layer session failures (D-spec error stratification).
///
/// Maps into the existing chain via `From<CrawlSessionError> for ScraperError`
/// — no new level (the `AppError` tier in AGENTS.md/README.md has zero
/// production hits; inventing it here would match a stale doc). Every variant
/// gains a row in `docs/error-classification-matrix.md` in the same change.
#[derive(Debug, Error)]
pub(crate) enum CrawlSessionError {
    /// Run description failed validation — fail before any worker spawns.
    #[error("invalid crawl session: {0}")]
    InvalidConfiguration(String),
    /// Checkpoint directory unwritable — degrades to no-checkpoint (logged).
    #[error("checkpoint unwritable at {path}: {reason}")]
    CheckpointUnwritable {
        /// Directory that could not be created.
        path: PathBuf,
        /// OS error text.
        reason: String,
    },
    /// Run cancelled through the session authority (surfaces only if a
    /// future slice lets cancellation escape engine control flow; today it
    /// is consumed there first — same defensive precedent as
    /// `CrawlError::Cancelled`).
    #[error("crawl cancelled")]
    #[allow(dead_code)] // slice 2: constructed by the shutdown-verdict path
    Cancelled,
    /// Session invariant broken (bug indicator).
    #[error("crawl session internal error: {0}")]
    #[allow(dead_code)] // slice 2: constructed by future validation branches
    Internal(String),
}

impl From<CrawlSessionError> for ScraperError {
    fn from(err: CrawlSessionError) -> Self {
        match err {
            // PermanentFatal / exit 78 via the Config row (matrix family 4).
            CrawlSessionError::InvalidConfiguration(msg) => ScraperError::Config(msg),
            CrawlSessionError::CheckpointUnwritable { path, reason } => ScraperError::Config(
                format!("checkpoint unwritable at {}: {reason}", path.display()),
            ),
            // Defensive only: cancellation is consumed by engine control flow
            // before any error surfaces (same precedent as CrawlError::Cancelled).
            CrawlSessionError::Cancelled => {
                ScraperError::Internal("crawl cancelled by session authority".to_string())
            },
            CrawlSessionError::Internal(msg) => ScraperError::Internal(msg),
        }
    }
}

impl From<CrawlSessionError> for CrawlError {
    fn from(err: CrawlSessionError) -> Self {
        // Row 33 (P6-2 slice 2): the rendered session error travels as the
        // payload; upstream maps to `ScraperError::Config` (exit 78).
        CrawlError::InvalidSession(err.to_string())
    }
}
