//! Shared base for the downloader tests.

use super::*;

/// Helper: parse de ValidUrl (#1117).
pub(super) fn vu(s: &str) -> ValidUrl {
    ValidUrl::parse(s).expect("test url is valid")
}
