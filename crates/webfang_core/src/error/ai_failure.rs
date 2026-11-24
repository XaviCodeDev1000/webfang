//! Wire contract between AI adapters and the error funnel (S11/T6).
//!
//! [`SemanticError::Inference`] is deliberately an unstructured string: the
//! port contract stays unchanged and no new error variants are introduced.
//! Remote adapters append a small, stable machine-readable tag to that
//! string; the single `From<SemanticError> for ScraperError` conversion reads
//! it and maps an OBSERVED remote failure onto the existing
//! [`ScraperError`] taxonomy. An untagged inference is a local consistency
//! failure and keeps its `Semantic` variant (InternalFatal).
//!
//! The tag is appended after the human prose so the operator-facing message
//! survives unchanged and so existing substring assertions keep working.

use super::{ScraperError, SemanticError};

const TAG_SEPARATOR: &str = "\n";
const TAG_PREFIX: &str = "[ai-remote ";
const TAG_END: &str = "]";

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteKind {
    Status {
        status: u16,
        url: String,
        retry_after: Option<u64>,
    },
    Transport,
}

fn tag_for(kind: &RemoteKind) -> String {
    match kind {
        RemoteKind::Status {
            status,
            url,
            retry_after,
        } => match retry_after {
            Some(secs) => format!("[ai-remote status={status} url={url} retry={secs}]"),
            None => format!("[ai-remote status={status} url={url}]"),
        },
        RemoteKind::Transport => "[ai-remote transport]".to_owned(),
    }
}

/// Build a `SemanticError` that carries an OBSERVED HTTP status.
///
/// The adapter knows the endpoint it dialed and the `Retry-After` it honored;
/// both travel in the tag so the conversion never has to guess a status that
/// was not actually seen on the wire.
#[must_use]
pub fn remote_status(
    message: impl Into<String>,
    status: u16,
    url: &str,
    retry_after: Option<u64>,
) -> SemanticError {
    let kind = RemoteKind::Status {
        status,
        url: url.to_owned(),
        retry_after,
    };
    SemanticError::Inference(format!(
        "{}{TAG_SEPARATOR}{}",
        message.into(),
        tag_for(&kind)
    ))
}

/// Build a `SemanticError` for a timeout or a transport failure.
#[must_use]
pub fn remote_transport(message: impl Into<String>) -> SemanticError {
    SemanticError::Inference(format!(
        "{}{TAG_SEPARATOR}{}",
        message.into(),
        tag_for(&RemoteKind::Transport)
    ))
}

fn parse_kind(inner: &str) -> Option<RemoteKind> {
    let mut tokens = inner.split_whitespace();
    let first = tokens.next()?;
    if first == "transport" && tokens.next().is_none() {
        return Some(RemoteKind::Transport);
    }
    let (key, value) = first.split_once('=')?;
    if key != "status" {
        return None;
    }
    let status = Some(value.parse::<u16>().ok()?);
    let mut url = None;
    let mut retry_after = None;
    for token in tokens {
        let (key, value) = token.split_once('=')?;
        match key {
            "url" => url = Some(value.to_owned()),
            "retry" => retry_after = Some(value.parse::<u64>().ok()?),
            _ => return None,
        }
    }
    Some(RemoteKind::Status {
        status: status?,
        url: url?,
        retry_after,
    })
}

/// The single conversion point for AI failures.
///
/// A malformed or unknown tag is treated as a local failure instead of being
/// silently reclassified: an adapter that cannot describe what it observed
/// must not gain a retry class by accident.
impl From<SemanticError> for ScraperError {
    fn from(err: SemanticError) -> Self {
        let SemanticError::Inference(message) = &err else {
            return Self::Semantic(err);
        };
        let Some((prose, tail)) = message.rsplit_once('\n') else {
            return Self::Semantic(err);
        };
        let Some(inner) = tail
            .strip_prefix(TAG_PREFIX)
            .and_then(|s| s.strip_suffix(TAG_END))
        else {
            return Self::Semantic(err);
        };
        match parse_kind(inner) {
            Some(RemoteKind::Status {
                status,
                url,
                retry_after,
            }) => {
                tracing::debug!(
                    status,
                    endpoint = %url,
                    retry_after_secs = retry_after.unwrap_or(0),
                    classification = if status == 429 { "rate_limited" } else { "http" },
                    "ai remote failure classified at the adapter boundary"
                );
                if status == 429 {
                    Self::RateLimited(retry_after.unwrap_or(0))
                } else {
                    Self::Http { status, url }
                }
            },
            Some(RemoteKind::Transport) => {
                tracing::debug!(
                    reason = "transport",
                    "ai remote failure classified at the adapter boundary"
                );
                Self::Network(Box::new(std::io::Error::other(prose.to_owned())))
            },
            None => Self::Semantic(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_tag_is_a_local_failure() {
        let err = SemanticError::Inference("local\n[ai-remote status=not-a-number url=x]".into());
        let scraper: ScraperError = err.into();
        assert!(matches!(scraper, ScraperError::Semantic(_)));
        assert_eq!(
            scraper.classify(),
            crate::domain::error::ErrorClass::InternalFatal
        );
    }

    #[test]
    fn tag_never_leaks_into_the_displayed_http_error() {
        let err = remote_status(
            "endpoint returned 503",
            503,
            "https://api.example.test/v1",
            None,
        );
        let scraper: ScraperError = err.into();
        assert!(!scraper.to_string().contains("ai-remote"));
    }
}
