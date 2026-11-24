//! #2836-derived pins for the cross-platform connection-error collapse.
//!
//! These live in their own target because `webfang_test_utils/src/lib.rs` is
//! grandfathered by the file-size ratchet and may not grow even one line.

use std::path::Path;
use webfang_test_utils::redact_nondeterministic;

/// The Spanish transport prefix exists only on the `io::Error` path (Linux);
/// Windows degrades to a timeout with no prefix. Both must land on one token.
#[test]
fn issue_2836_collapses_the_spanish_transport_prefix() {
    let dir = Path::new("/tmp/test");
    let input = "error de red: I/O error: el servidor rechazó la conexión: Connection refused (os error 111)";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "error de red: <NET_ERR>");
}

/// #3030: the cause taxonomy is now English, but the product-owned prefix is
/// still OS-dependent. Linux keeps it on the `io::Error` path; Windows
/// degrades to a timeout with no prefix. Both must land on `<NET_ERR>` or the
/// committed snapshot bakes a Linux-only string and the Windows lane goes red.
#[test]
fn issue_3030_collapses_the_english_transport_prefix() {
    let dir = Path::new("/tmp/test");
    let input = "network error: I/O error: the server refused the connection: Connection refused (os error 111)";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "network error: <NET_ERR>");

    let windows = redact_nondeterministic(dir, "network error: request timed out after 2s");
    assert_eq!(windows, "network error: <NET_ERR>");
}

/// The legacy Spanish spelling stays accepted: fixtures and older traces can
/// still carry it, and the collapse must not fork by language.
#[test]
fn issue_3030_keeps_the_spanish_transport_prefix_collapsed() {
    let dir = Path::new("/tmp/test");
    let input = "error de red: I/O error: el servidor rechazó la conexión: Connection refused (os error 111)";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "error de red: <NET_ERR>");
}

/// A dropped SYN reads `connect_refused` on Linux and `connect_timeout` on
/// Windows; the classification contract is pinned by domain tests, not here.
#[test]
fn issue_2836_normalizes_the_connection_cause() {
    let dir = Path::new("/tmp/test");
    for input in ["cause: \"connect_refused\"", "cause: \"connect_timeout\""] {
        assert_eq!(
            redact_nondeterministic(dir, input),
            "cause: \"<NET_CAUSE>\""
        );
    }
}

/// The full Linux and Windows error lines must redact to identical text.
#[test]
fn issue_2836_makes_both_connect_failures_agree() {
    let dir = Path::new("/tmp/test");
    let linux = redact_nondeterministic(
        dir,
        "error de red: I/O error: el servidor rechazó la conexión: Connection refused (os error 111), cause: \"connect_refused\"",
    );
    let windows = redact_nondeterministic(
        dir,
        "error de red: request timed out after 2s, cause: \"connect_timeout\"",
    );
    assert_eq!(linux, windows, "both platforms must redact identically");
    assert_eq!(linux, "error de red: <NET_ERR>, cause: \"<NET_CAUSE>\"");
}
