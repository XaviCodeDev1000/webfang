use super::*;
use serde_json::{json, Value};

#[test]
fn require_safe_path_allow_absolute_accepts_absolute() {
    // Bug #8 regression: absolute paths MUST be accepted (issue #590).
    let result = require_safe_path_allow_absolute("vault_path", "/home/user/vault");
    assert!(result.is_ok(), "absolute path must be accepted: {result:?}");
    assert_eq!(result.unwrap().to_string_lossy(), "/home/user/vault");
}

#[test]
fn require_safe_path_allow_absolute_rejects_traversal() {
    // `..` traversal must still be rejected even for absolute paths.
    let result = require_safe_path_allow_absolute("vault_path", "/home/user/../etc/passwd");
    assert!(result.is_err(), "traversal must be rejected: {result:?}");
}

#[test]
fn require_safe_path_allow_absolute_rejects_empty() {
    let result = require_safe_path_allow_absolute("vault_path", "");
    assert!(result.is_err(), "empty path must be rejected: {result:?}");
}

#[test]
fn require_safe_path_allow_absolute_accepts_relative() {
    // Relative paths should still work (no regression).
    let result = require_safe_path_allow_absolute("vault_path", "my-vault");
    assert!(result.is_ok(), "relative path must be accepted: {result:?}");
}

// --- require_safe_path drive-relative probe (#1588) --------------------

#[test]
fn require_safe_path_rejects_drive_relative_without_separator() {
    // #1588: `C:foo` is drive-relative (relative to the current directory
    // ON the drive) — an escape on Windows hosts, never a safe relative
    // path. The old separator-requiring probe let it through on every host.
    for value in ["C:foo", "c:foo", "C:", "D:exports/2026"] {
        let err = require_safe_path("file_path", value).unwrap_err();
        assert!(
            matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS),
            "{value} must be rejected as drive-relative"
        );
    }
}

#[test]
fn require_safe_path_allow_absolute_keeps_accepting_absolute_windows_paths() {
    // The drive-prefix change must NOT leak into the allow-absolute
    // variant: `C:\vault` stays valid for `detect_obsidian_vault` (#590).
    let result = require_safe_path_allow_absolute("vault_path", "C:\\vault");
    assert!(
        result.is_ok(),
        "absolute Windows path must be accepted: {result:?}"
    );
}

// --- require_safe_filename (issue #601) ---------------------------------

#[test]
fn require_safe_filename_accepts_flat_name() {
    assert!(require_safe_filename("filename", "doc").is_ok());
    assert!(require_safe_filename("filename", "report-2026.json").is_ok());
    assert!(require_safe_filename("filename", "a.b.c").is_ok());
}

#[test]
fn require_safe_filename_rejects_parent_traversal() {
    // The exact payload from issue #601.
    assert!(require_safe_filename("filename", "../escape").is_err());
    assert!(require_safe_filename("filename", "sub/../escape").is_err());
    assert!(require_safe_filename("filename", "..").is_err());
}

#[test]
fn require_safe_filename_rejects_subdirectory() {
    // `sub/out` must not silently create nested directories.
    assert!(require_safe_filename("filename", "sub/out").is_err());
    assert!(require_safe_filename("filename", "a/b/c").is_err());
}

#[test]
fn require_safe_filename_rejects_separators_and_root() {
    assert!(require_safe_filename("filename", "/etc/passwd").is_err());
    assert!(require_safe_filename("filename", ".\\windows").is_err());
    assert!(require_safe_filename("filename", "").is_err());
    assert!(require_safe_filename("filename", ".").is_err());
}

#[test]
fn sanitized_filename_newtype_rejects_traversal() {
    assert!("sub/out".parse::<SanitizedFilename>().is_err());
    assert!("..".parse::<SanitizedFilename>().is_err());
    let ok = "doc".parse::<SanitizedFilename>().expect("flat name valid");
    assert_eq!(ok.as_str(), "doc");
}

// --- cross-platform filename hardening (issue #1608) --------------------

#[test]
fn require_safe_filename_accepts_plain_cross_platform_names() {
    // Sanity: the hardening must not reject ordinary names.
    assert!(require_safe_filename("filename", "documento final").is_ok());
    assert!(require_safe_filename("filename", "report.2026-09-26.json").is_ok());
    assert!(require_safe_filename("filename", "naïve-file_λ").is_ok());
}

#[test]
fn require_safe_filename_rejects_control_characters() {
    // NUL, C0 escapes, DEL and C1 are all Unicode Cc — XP-P-01-adjacent.
    assert!(require_safe_filename("filename", "doc\u{0}ument").is_err());
    assert!(require_safe_filename("filename", "doc\u{1b}[31m").is_err());
    assert!(require_safe_filename("filename", "doc\u{7f}").is_err());
    assert!(require_safe_filename("filename", "doc\u{85}").is_err());
}

#[test]
fn require_safe_filename_rejects_windows_reserved_names() {
    // XP-P-04: stem-aware, case-insensitive — `CON.txt` is as unusable on
    // Windows as `CON`.
    for value in [
        "CON",
        "con",
        "Con",
        "CON.txt",
        "nul.tar.gz",
        "PRN",
        "AUX",
        "COM1",
        "lpt9",
    ] {
        assert!(
            require_safe_filename("filename", value).is_err(),
            "'{value}' must be rejected as a Windows reserved name"
        );
    }
    // Lookalikes that are NOT reserved stay accepted.
    assert!(require_safe_filename("filename", "console").is_ok());
    assert!(require_safe_filename("filename", "com10").is_ok());
}

#[test]
fn require_safe_filename_rejects_colon_ads_hazard() {
    // XP-P-05: any `:` in a flat filename is an NTFS alternate-data-stream
    // hazard; rejected on every host for cross-platform consistency.
    assert!(require_safe_filename("filename", "a:b").is_err());
    assert!(require_safe_filename("filename", "stream.txt:ads").is_err());
    // A drive prefix is NOT a filename exception either.
    assert!(require_safe_filename("filename", "C:").is_err());
}

#[test]
fn require_safe_filename_rejects_trailing_dot_or_space() {
    // XP-P-06: rejected as-is — never silently trimmed.
    assert!(require_safe_filename("filename", "documento final.").is_err());
    assert!(require_safe_filename("filename", "documento final ").is_err());
    // Mid-name dots/spaces stay fine.
    assert!(require_safe_filename("filename", "a.b c").is_ok());
}

#[test]
fn require_safe_filename_rejects_windows_invalid_charset() {
    for value in ["a<b", "a>b", "a\"b", "a|b", "a?b", "a*b"] {
        assert!(
            require_safe_filename("filename", value).is_err(),
            "'{value}' must be rejected as Windows-invalid"
        );
    }
}

#[test]
fn require_safe_filename_rejects_oversize_component() {
    // XP-P-07 partial: a single component over 255 bytes is rejected even
    // though the whole value is still under MAX_PATH_LEN.
    let long = "a".repeat(256);
    assert!(
        long.len() < MAX_PATH_LEN,
        "fixture must be under the 1 KiB cap"
    );
    assert!(require_safe_filename("filename", &long).is_err());
    let ok = "a".repeat(255);
    assert!(require_safe_filename("filename", &ok).is_ok());
}

#[test]
fn require_safe_path_applies_component_checks() {
    // Issue #1608: the same filename-level rules per path component.
    assert!(require_safe_path("file_path", "notes/CON.md").is_err());
    assert!(require_safe_path("file_path", "notes/a:b.md").is_err());
    assert!(require_safe_path("file_path", "notes/doc.. ").is_err());
    assert!(require_safe_path("file_path", "notes/trailing.").is_err());
    assert!(require_safe_path("file_path", "notes/a<b.md").is_err());
    // Benign nested paths stay accepted.
    assert!(require_safe_path("file_path", "notes/2026/09/doc.md").is_ok());
}

#[test]
fn require_safe_path_allow_absolute_applies_component_checks() {
    assert!(require_safe_path_allow_absolute("vault_path", "/vault/CON.md").is_err());
    assert!(require_safe_path_allow_absolute("vault_path", "/vault/a|b.md").is_err());
    // The #590 contract survives: absolute Windows paths with a drive
    // prefix stay valid — the drive colon is a separator, not an ADS.
    assert!(require_safe_path_allow_absolute("vault_path", "C:\\vault").is_ok());
}

// --- EC-08 reason-slug contract (issue #1613) ---------------------------
//
// The transport-level proof lives in
// `tests/mcp_validation_reason_code_test.rs` (a live server, real
// JSON-RPC `error.data`). These cover what the transport cannot reach:
// the four slugs no tool argument can provoke, the branches behind a
// typed boundary, and the properties that make the taxonomy load-bearing
// — the closed 7-value set, the `data` SHAPE, and the fact that adding
// an 8th slug is a visible act.
// -----------------------------------------------------------------------

/// Every slug the module can emit.
const ALL_REASONS: [&str; 7] = [
    REASON_EMPTY,
    REASON_TOO_LONG,
    REASON_MALFORMED,
    REASON_UNSUPPORTED_SCHEME,
    REASON_PATH_NOT_ALLOWED,
    REASON_OUT_OF_RANGE,
    REASON_NOT_IN_ALLOWED_SET,
];

/// The `(field, reason)` pair carried by a rejection.
fn tag(err: &McpError) -> (String, Option<String>) {
    let data = err.data.as_ref().expect("every rejection carries `data`");
    assert!(
        data.is_object(),
        "`data` must be a JSON OBJECT now, not a bare string: {data}"
    );
    (
        data.get("field")
            .and_then(Value::as_str)
            .expect("`data.field` must be the offending field")
            .to_string(),
        data.get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

#[test]
fn reason_slugs_are_distinct_and_the_set_is_closed() {
    for (i, a) in ALL_REASONS.iter().enumerate() {
        for b in &ALL_REASONS[i + 1..] {
            assert_ne!(a, b, "two rules must not share a reason slug");
        }
    }
    // The taxonomy is a wire contract: a typo here is a slug no caller
    // branches on. This assertion is the tripwire for that.
    assert_eq!(ALL_REASONS.len(), 7, "the taxonomy has exactly 7 slugs");
}

#[test]
fn every_rejection_helper_emits_a_slug_from_the_closed_set() {
    let errs = [
        require_http_url("url", "").unwrap_err(),
        require_http_url("url", &"x".repeat(MAX_URL_LEN + 1)).unwrap_err(),
        require_http_url("url", "not a url").unwrap_err(),
        require_http_url("url", "ftp://example.com").unwrap_err(),
        require_safe_path("file_path", "").unwrap_err(),
        require_safe_path("file_path", &"x".repeat(MAX_PATH_LEN + 1)).unwrap_err(),
        require_safe_path("file_path", "/etc/passwd").unwrap_err(),
        require_safe_path("file_path", "notes/../escape").unwrap_err(),
        require_safe_path("file_path", "notes/CON.md").unwrap_err(),
        require_safe_path_allow_absolute("vault_path", "").unwrap_err(),
        require_safe_path_allow_absolute("vault_path", "v/../x").unwrap_err(),
        require_max_len("html", "xx", 1).unwrap_err(),
        require_safe_name("vault_name", "").unwrap_err(),
        require_safe_domain("base_domain", "").unwrap_err(),
        require_safe_domain("base_domain", "exa mple.com").unwrap_err(),
        require_safe_domain("base_domain", "example.com/../x").unwrap_err(),
        require_safe_seed("seed_domain", "").unwrap_err(),
        require_safe_seed("seed_domain", "example.com/x").unwrap_err(),
        require_safe_seed("seed_domain", "example.com/../x").unwrap_err(),
        require_safe_seed("seed_domain", "ftp://example.com").unwrap_err(),
        require_non_empty("query", "").unwrap_err(),
        require_max_value_u64("urls", 2, 1).unwrap_err(),
        require_range_u64("concurrency", 0, 1, 5).unwrap_err(),
        require_max_value_u16("retries", 9, 3).unwrap_err(),
        require_safe_filename("filename", "").unwrap_err(),
        require_safe_filename("filename", "../escape").unwrap_err(),
        require_safe_filename("filename", "CON").unwrap_err(),
        require_one_of("content_format", "yaml", &["jsonl", "vector"]).unwrap_err(),
    ];
    for err in &errs {
        let (_, reason) = tag(err);
        let slug = reason.expect("every helper rejection must carry a reason slug");
        assert!(
            ALL_REASONS.contains(&slug.as_str()),
            "'{slug}' is outside the documented 7-value taxonomy: {err:?}"
        );
    }
}

#[test]
fn each_helper_emits_the_slug_its_rule_means() {
    // One representative per slug, so a wrong MAPPING (not a wrong shape)
    // is caught: each of these asserts the specific remediation the slug
    // promises.
    let cases: [(McpError, &str); 7] = [
        (require_http_url("url", "").unwrap_err(), REASON_EMPTY),
        (
            require_max_len("html", "xx", 1).unwrap_err(),
            REASON_TOO_LONG,
        ),
        (
            require_http_url("url", "http://").unwrap_err(),
            REASON_MALFORMED,
        ),
        (
            require_http_url("url", "ftp://example.com").unwrap_err(),
            REASON_UNSUPPORTED_SCHEME,
        ),
        (
            require_safe_path("file_path", "notes/../x").unwrap_err(),
            REASON_PATH_NOT_ALLOWED,
        ),
        (
            require_max_value_u64("max_depth", 11, 10).unwrap_err(),
            REASON_OUT_OF_RANGE,
        ),
        (
            require_one_of("content_format", "yaml", &["jsonl"]).unwrap_err(),
            REASON_NOT_IN_ALLOWED_SET,
        ),
    ];
    for (err, expected) in cases {
        assert_eq!(
            tag(&err).1.as_deref(),
            Some(expected),
            "wrong slug for: {err:?}"
        );
    }
}

#[test]
fn data_carries_the_offending_field_beside_the_slug() {
    // The field tag is the information the old bare-string `data` carried;
    // EC-08 keeps it so nothing is lost in the restructure.
    for (err, field) in [
        (
            require_http_url("url", "ftp://x.example").unwrap_err(),
            "url",
        ),
        (require_max_len("html", "xx", 1).unwrap_err(), "html"),
        (
            require_safe_filename("filename", "..").unwrap_err(),
            "filename",
        ),
    ] {
        assert_eq!(tag(&err).0, field, "wrong field tag for: {err:?}");
    }
}

#[test]
fn filename_component_messages_are_unchanged_and_tagged_path_not_allowed() {
    // The Spanish island stays byte-for-byte what it was (no reword, no
    // translate — #1613 documents the language contract, it does not unify
    // it); only the machine-readable half is added.
    let err = require_safe_filename("filename", "CON").unwrap_err();
    assert_eq!(
        err.message,
        "usa un nombre reservado de Windows (CON, PRN, AUX, NUL, COM1-9, LPT1-9)"
    );
    assert_eq!(tag(&err).1.as_deref(), Some(REASON_PATH_NOT_ALLOWED));

    // `a:b` parses as a drive-relative path on Windows (Prefix + Normal),
    // so the multi-component guard rejects it before the colon rule is
    // reached. The probe must be a single Normal component on BOTH
    // platforms for the message contract asserted here to hold.
    let err = require_safe_filename("filename", "stream.txt:ads").unwrap_err();
    assert_eq!(
        err.message,
        "no debe contener ':' (riesgo de flujos alternativos NTFS)"
    );
    assert_eq!(tag(&err).1.as_deref(), Some(REASON_PATH_NOT_ALLOWED));
}

#[test]
fn reason_of_tolerates_a_missing_or_foreign_data_payload() {
    // `data` is sender-defined, so the success-shaped `validate_url`
    // channel may see anything: absent, the pre-EC-08 bare string, or an
    // object without a `reason`. None of those may panic.
    assert_eq!(reason_of(None), None);
    assert_eq!(reason_of(Some(&Value::String("url".to_string()))), None);
    assert_eq!(reason_of(Some(&json!({"field": "url"}))), None);
    assert_eq!(reason_of(Some(&json!({"reason": 42}))), None);
    assert_eq!(
        reason_of(Some(&json!({"reason": REASON_MALFORMED}))),
        Some(REASON_MALFORMED)
    );
}
