//! Batch input parsing — line accounting for `--batch-file`/stdin sources.
//!
//! Split from `manager.rs` (#2795): the parse-and-account stage between the
//! raw file/stdin text and the manager's URL queue is self-contained, and
//! keeping it here keeps both files under the size guard. The manager calls
//! `parse_url_lines` and `warn_rejected_lines`; the CLI reads the
//! resulting [`BatchInputLines`] through the manager and the summary.

use crate::domain::ValidUrl;

/// Input accounting for one batch source (`--batch-file`/stdin) (#2795).
///
/// Every physical line the source yielded lands in exactly one category, so
/// the operator arithmetic `lines read = valid + rejected + blank + comment`
/// always closes. Blank lines and `#` comments are legal input, not
/// rejections; only [`Self::rejected`] counts the lines [`ValidUrl`] refused
/// at the entry (F-R3-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchInputLines {
    /// Physical lines the source yielded (`str::lines`; a trailing newline
    /// never adds a phantom line).
    pub lines_read: usize,
    /// Lines accepted as URLs after [`ValidUrl`] hardening.
    pub valid: usize,
    /// Lines [`ValidUrl`] rejected at the entry (F-R3-1).
    pub rejected: usize,
    /// Whitespace-only lines (legal separators, never rejections).
    pub blank: usize,
    /// `#` comment lines (legal, never rejections).
    pub comment: usize,
    /// `(line_number, reason)` per rejected line, 1-based physical order.
    /// The reason text never echoes the offending line (F-R3-1).
    pub rejected_details: Vec<(usize, String)>,
}

/// Log lines rejected at the batch entry (F-R3-1). Never echoes the
/// offending text — a malformed line can still carry credential-looking
/// content, and traces must not capture it. Each warning carries the
/// physical input line number so the operator can open the file and fix
/// it (#2795).
pub(crate) fn warn_rejected_lines(rejected: &[(usize, String)]) {
    for (line, reason) in rejected {
        tracing::warn!(line = line, reason = %reason, "batch line rejected by URL entry validation");
    }
    if !rejected.is_empty() {
        tracing::warn!(count = rejected.len(), "batch lines skipped");
    }
}

/// Parse URL lines from text, accounting every physical line (#2795)
///
/// Each kept line is hardened through [`ValidUrl`] at the batch entry
/// (scheme allow-list + credential strip, F-R3-1): the raw string is
/// replaced by its sanitized rendering before it enters any job, so
/// credentials from `--batch-file`/stdin never reach logs, traces, or
/// the processor. Blank lines and `#` comments are legal input counted in
/// their own categories; lines that fail `ValidUrl` are dropped and
/// reported with their 1-based line number.
pub(crate) fn parse_url_lines(content: &str) -> (Vec<String>, BatchInputLines) {
    let mut kept: Vec<String> = Vec::new();
    let mut stats = BatchInputLines::default();
    for (index, line) in content.lines().enumerate() {
        stats.lines_read += 1;
        let line = line.trim();
        if line.is_empty() {
            stats.blank += 1;
            continue;
        }
        if line.starts_with('#') {
            stats.comment += 1;
            continue;
        }
        match ValidUrl::parse(line) {
            Ok(valid) => {
                kept.push(valid.as_url().to_string());
                stats.valid += 1;
            },
            Err(e) => {
                stats.rejected += 1;
                stats.rejected_details.push((index + 1, e.to_string()));
            },
        }
    }
    (kept, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_lines() {
        let content = r#"
# comment line
https://example.com

https://example.com/about
  # indented comment
https://example.com/blog
"#;
        let (urls, stats) = parse_url_lines(content);
        assert_eq!(stats.rejected, 0);
        // #2795: every physical line lands in exactly one category, so the
        // operator arithmetic closes: 7 read = 3 valid + 0 rejected + 2 blank
        // + 2 comment. The fixture's trailing newline adds NO phantom line
        // (`str::lines`: the final line ending is optional).
        assert_eq!(
            (stats.lines_read, stats.valid, stats.blank, stats.comment),
            (7, 3, 2, 2)
        );
        // Url normalization adds the trailing slash to bare hosts.
        assert_eq!(urls[0], "https://example.com/");
        assert_eq!(urls[1], "https://example.com/about");
        assert_eq!(urls[2], "https://example.com/blog");
    }

    #[test]
    fn test_parse_url_lines_rejects_non_http_and_strips_credentials() {
        // F-R3-1: entry validation is uniform with the seed path (#1240).
        let (urls, stats) =
            parse_url_lines("https://user:secret@example.com/page\nfile:///etc/passwd\nnot a url");
        // The valid line survives stripped: no credential text anywhere.
        assert_eq!(urls, vec!["https://example.com/page".to_string()]);
        // Both invalid lines are rejected at the entry.
        assert_eq!(stats.rejected, 2);
        // Each rejection carries its 1-based physical line number (#2795).
        let line_numbers: Vec<usize> = stats
            .rejected_details
            .iter()
            .map(|(line, _)| *line)
            .collect();
        assert_eq!(line_numbers, vec![2, 3]);
        // Rejection reasons never echo the offending line.
        for (_, reason) in &stats.rejected_details {
            assert!(!reason.contains("file:"));
            assert!(!reason.contains("not a url"));
        }
    }

    #[test]
    fn test_parse_empty() {
        let (urls, stats) = parse_url_lines("");
        assert!(urls.is_empty());
        assert_eq!(stats, BatchInputLines::default());
    }

    #[test]
    fn test_parse_only_comments() {
        let (urls, stats) = parse_url_lines("# just a comment\n# another");
        assert!(urls.is_empty());
        // Comments are legal input in their own category, never rejections.
        assert_eq!(
            (stats.lines_read, stats.valid, stats.rejected, stats.comment),
            (2, 0, 0, 2)
        );
    }
}
