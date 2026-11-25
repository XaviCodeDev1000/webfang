//! OS-dependent connection-failure collapse shared by every redacted snapshot.
//!
//! Kept in its own module: the connection-collapse regex rationale is a
//! self-contained unit owned by the redaction family.

use regex::Regex;
use std::sync::LazyLock;

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
/// phrase BEFORE the OS text, present only on the `io::Error` path. It belongs
/// to the same collapse, or the committed snapshot can only ever match one OS.
/// The classified cause is equally OS-dependent: a dropped SYN reads
/// `connect_refused` on Linux and `connect_timeout` on Windows, while the
/// classification contract itself is pinned by domain tests.
///
/// #3030: `NetworkCause::message()` changed that phrase from Spanish to
/// English. The collapse must accept BOTH spellings: on Linux the English
/// phrase stays on the `io::Error` path, while Windows degrades to a timeout
/// with no phrase at all. Leaving only the Spanish spelling in the regex made
/// the snapshot bake a Linux-only prefix and broke the Windows lane.
#[must_use]
pub(crate) fn collapse_connection_surface(text: String) -> String {
    // Compiled once: the regex crate documents repeated compilation inside a
    // helper as an anti-pattern, and this chain runs on every redacted snapshot.
    static NET_ERR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:I/O error:\s*)?(?:(?:el servidor rechazó la conexión|the server refused the connection):\s*)?(?:(?:connection refused|connection timed out|no connection could be made[^()\n]*|an attempt to connect[^()\n]*)\s*(?:\(\s*os error\s*\d+\s*\))?|request timed out after \d+\s*s)",
        )
        .expect("valid network error regex")
    });
    static NET_CAUSE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"cause: "(?:connect_refused|connect_timeout)""#)
            .expect("valid network cause regex")
    });
    let text = NET_ERR.replace_all(&text, "<NET_ERR>").into_owned();
    NET_CAUSE
        .replace_all(&text, r#"cause: "<NET_CAUSE>""#)
        .into_owned()
}

/// Collapse the platform-dependent resolver payload after webfang's own
/// `DNS error:` wrapper to ONE token.
///
/// A comma is not a safe boundary: macOS `getaddrinfo` reports EAI_NONAME as
/// "nodename nor servname provided, or not known", so stopping at the first
/// comma used to leave ", or not known" in the snapshot. The payload ends at
/// `, url: ` (error line), `]` (`[Failed: ...]` summary), ` at ` (the retry
/// WARN), a newline (the final `Failed to scrape ...` line), or end of text.
/// `regex` has no lookahead, so the delimiter is captured and re-emitted by
/// the replacement.
///
/// This lived in one CLI test file; keeping it here means every snapshot that
/// captures a resolution failure gets the same rule without opting in.
#[must_use]
pub(crate) fn collapse_resolver_payload(text: String) -> String {
    static RESOLVER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(DNS error: )[^\]\n]*?(, url: |\]|\s+at\s|\n|$)")
            .expect("valid resolver payload regex")
    });
    RESOLVER
        .replace_all(&text, "${1}<RESOLVER>${2}")
        .into_owned()
}
