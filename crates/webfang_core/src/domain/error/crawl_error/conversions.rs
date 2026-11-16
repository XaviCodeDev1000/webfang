//! `HttpError` → `CrawlError` (#2784).
//!
//! The conversion lives in its own module so `crawl_error.rs` stays under its
//! grandfathered size: the variant it maps to now carries the classified cause.

use super::CrawlError;
use super::WafDetectionKind;

impl From<crate::domain::http_error::HttpError> for CrawlError {
    fn from(e: crate::domain::http_error::HttpError) -> Self {
        use crate::domain::http_error::HttpError;
        match e {
            HttpError::Forbidden => CrawlError::Http {
                status: 403,
                url: String::new(),
            },
            HttpError::RateLimited(retry_after) => CrawlError::RateLimited(retry_after),
            HttpError::ClientError(code) => CrawlError::Http {
                status: code,
                url: String::new(),
            },
            HttpError::ServerError(code) => CrawlError::Http {
                status: code,
                url: String::new(),
            },
            HttpError::Timeout { source } => CrawlError::Timeout { source },
            HttpError::Connection { message, source } => CrawlError::Connection { message, source },
            HttpError::Request(msg) => CrawlError::RequestFailed(msg),
            // #2841: `Request` is reserved for request CONSTRUCTION failures; a
            // transport failure the classifier could not name keeps its cause.
            HttpError::Transport { message, source } => CrawlError::Transport { message, source },
            HttpError::WafChallenge(provider) => CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                url: String::new(),
            },
            HttpError::DomainBanned(domain) => {
                CrawlError::SessionPool(format!("domain banned: {domain}"))
            },
            HttpError::BodyTooLarge { limit } => CrawlError::RequestFailed(format!(
                "response body exceeded the limit of {limit} bytes"
            )),
        }
    }
}
