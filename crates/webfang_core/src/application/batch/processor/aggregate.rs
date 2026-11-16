//! The batch's aggregate error for a run that failed with transient errors.
//!
//! #2824: the run classified its transport failure and the aggregate keeps that
//! cause in its chain instead of dropping it. The failures themselves are
//! COUNTED, not kept — the cause is what the operator needs to read ("the
//! server refused the connection", not "some errors"), and it is the cause the
//! transport already classified.

use crate::domain::error::cause::ClassifiedNetworkError;
use crate::domain::{CrawlError, CrawlResult};

/// The error the batch reports for a run that failed with transient errors.
///
/// The run classified its transport failure: the aggregate keeps that cause in
/// its chain (#2824) instead of dropping it. The failures themselves are
/// COUNTED, not kept — the cause is what the operator needs to read ("the
/// server refused the connection", not "some errors"), and it is the cause
/// the transport already classified.
pub(super) fn aggregate_transient_failure(result: &CrawlResult) -> CrawlError {
    let detail = format!("crawl completed with {} transient error(s)", result.errors);
    match result.transport_cause {
        Some(cause) => CrawlError::Connection {
            message: detail.clone(),
            source: Some(Box::new(ClassifiedNetworkError::new(
                cause,
                Box::new(std::io::Error::other(detail)),
            ))),
        },
        None => CrawlError::Connection {
            message: detail,
            source: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::cause::{cause_in_chain, NetworkCause};

    /// #2824: the batch's aggregate error keeps the cause the run classified.
    ///
    /// The failures are counted, not kept — but the CAUSE is the diagnosis, and
    /// an aggregate that dropped it reported "some errors" where the operator
    /// could read "the server refused the connection".
    #[test]
    fn the_aggregate_error_carries_the_classified_cause() {
        let result =
            crate::domain::CrawlResult::new(vec![], 0, 3, std::collections::BTreeMap::new())
                .with_transport_cause(NetworkCause::ConnectRefused);

        let error = aggregate_transient_failure(&result);
        assert_eq!(
            cause_in_chain(&error),
            Some(NetworkCause::ConnectRefused),
            "the aggregate must carry the classified cause: {error}"
        );
    }

    /// #2824: a run with no classified cause keeps the aggregate honest — the
    /// error carries NO cause rather than a guessed one.
    #[test]
    fn the_aggregate_error_carries_no_cause_when_the_run_classified_none() {
        let result =
            crate::domain::CrawlResult::new(vec![], 0, 3, std::collections::BTreeMap::new());

        let error = aggregate_transient_failure(&result);
        assert_eq!(
            cause_in_chain(&error),
            None,
            "an unclassified run must not invent a cause: {error}"
        );
    }
}
