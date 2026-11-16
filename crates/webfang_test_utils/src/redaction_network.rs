//! OS-dependent connection-failure collapse shared by every redacted snapshot.
//!
//! Kept in its own module because `lib.rs` is grandfathered by the file-size
//! ratchet and may not grow even one line.

use regex::Regex;

/// Collapse the connection-failure surface to ONE token.
///
/// INT-1 (#1631): Unix reports `I/O error: Connection refused (os error 111)`,
/// Windows reports our own `request timed out after 2s` (a refused connection
/// never happens there — it degrades to the request timeout), and a Windows
/// WSA refusal reads `No connection could be made ... (os error 10061)`.
/// Redaction cannot bridge a different error, so the timeout tail collapses
/// to the same token.
///
/// The `I/O error: ` layer is part of the SAME collapse: it is our
/// `DownloadError::Io` wrapper, present only when the inner error is an
/// `io::Error`. On Unix that error carries the connection failure; on the
/// timeout path the error is `DownloadError::Timeout`, with no wrapper.
///
/// The token records THAT a network failure happened, not WHICH one: the
/// affected tests assert that a failure is mentioned, not its kind.
///
/// #2836 follow-up: the connection-cause taxonomy added a product-owned
/// Spanish phrase BEFORE the OS text, present only on the `io::Error` path.
/// It belongs to the same collapse, or the committed snapshot can only ever
/// match one OS. The classified cause is equally OS-dependent: a dropped SYN
/// reads `connect_refused` on Linux and `connect_timeout` on Windows, while
/// the classification contract itself is pinned by domain tests.
#[must_use]
pub(crate) fn collapse_connection_surface(text: String) -> String {
    let net_err = Regex::new(
        r"(?i)(?:I/O error:\s*)?(?:el servidor rechazó la conexión:\s*)?(?:(?:connection refused|connection timed out|no connection could be made[^()\n]*|an attempt to connect[^()\n]*)\s*(?:\(\s*os error\s*\d+\s*\))?|request timed out after \d+\s*s)",
    )
    .expect("valid network error regex");
    let text = net_err.replace_all(&text, "<NET_ERR>").into_owned();
    let net_cause = Regex::new(r#"cause: "(?:connect_refused|connect_timeout)""#)
        .expect("valid network cause regex");
    net_cause
        .replace_all(&text, r#"cause: "<NET_CAUSE>""#)
        .into_owned()
}
