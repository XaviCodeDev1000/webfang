//! Filesystem-safety validation for the MCP tools: the safe-path and
//! flat-filename gates (`require_safe_path`, `require_safe_path_allow_absolute`,
//! `require_safe_filename`, the `SanitizedFilename` newtype) plus the
//! shared per-component hardening they all funnel through. Every rejection
//! carries the `path_not_allowed` slug from the parent module's reason
//! taxonomy; the limits live with the parent (`MAX_PATH_LEN`).

use rmcp::ErrorData as McpError;
use std::path::{Component, Path, PathBuf};
use webfang_core::domain::crawler_port::filename::is_windows_reserved;

use super::{
    invalid_params_with_reason, MAX_PATH_LEN, REASON_EMPTY, REASON_PATH_NOT_ALLOWED,
    REASON_TOO_LONG,
};

/// Max bytes per single filename component (issue #1608, XP-P-07 partial).
/// ext4 caps at 255; we target the lowest common denominator so a name
/// accepted here is creatable on every supported filesystem. Long-name
/// mitigation (`\\?\` extended-length prefixes, manifests) is explicitly out
/// of scope.
const MAX_COMPONENT_BYTES: usize = 255;

/// Validate that `value` is a safe filesystem path: non-empty, ≤
/// [`MAX_PATH_LEN`], relative (no leading `/`, no Windows drive letter), and
/// free of `..` traversal components.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty, oversize, absolute, or
/// `..`-traversal paths — [`REASON_EMPTY`], [`REASON_TOO_LONG`], or
/// [`REASON_PATH_NOT_ALLOWED`] respectively. Every path-level rule shares the
/// one `path_not_allowed` slug on purpose: what the caller must DO (switch to a
/// safe relative path) is identical for all of them, and the message carries
/// the specifics.
pub fn require_safe_path(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let path = Path::new(value);
    // Reject absolute paths. `Path::is_absolute()` is platform-aware (returns
    // false for `C:\Windows` on Unix), so also probe the string for leading
    // slashes, UNC prefixes, and Windows drive letters explicitly.
    if value.starts_with('/') || value.starts_with('\\') {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no leading '/' or UNC prefix)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if has_windows_drive_prefix(value) {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no Windows drive letter)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if path.is_absolute() {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no leading '/' or Windows drive letter)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..' traversal components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Per-component filename hardening (issue #1608): the relative-only
    // contract means no drive prefix is legitimate anywhere in the value.
    if let Some(reason) = normal_components(path).find_map(|c| filename_component_error(&c, false))
    {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    Ok(path.to_path_buf())
}

/// The `Normal` components of `path` as lossy strings — the only components
/// the filename-level checks apply to (structural kinds are handled by the
/// callers' own rules).
fn normal_components(path: &Path) -> impl Iterator<Item = std::borrow::Cow<'_, str>> {
    path.components().filter_map(|c| match c {
        Component::Normal(os) => Some(os.to_string_lossy()),
        _ => None,
    })
}

/// Detect a Windows-style drive-letter prefix (letter + `:`) regardless of
/// host platform. Case-insensitive letter; NO separator requirement — the
/// drive-relative `C:foo` form (relative to the current directory ON drive
/// C:, an escape on Windows hosts) must be rejected by
/// [`require_safe_path`]'s relative-only contract just like `C:\foo` (#1588).
/// `require_safe_path_allow_absolute` deliberately does not call this: it
/// accepts absolute Windows paths (`C:\vault`).
fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Filename-level hardening shared by [`require_safe_filename`] and the
/// per-component extension of [`require_safe_path`] /
/// [`require_safe_path_allow_absolute`] (issue #1608).
///
/// Returns the rejection MESSAGE only — every caller attaches
/// [`REASON_PATH_NOT_ALLOWED`], because the caller's remedy ("use a plain
/// filename inside a safe relative path") is the same for all of them, while
/// the message carries the specific rule that fired.
///
/// Checks one path component (no separators can occur inside a
/// `Component::Normal`) and returns the Spanish rejection reason for the
/// first violated rule, or `None` when the component is safe:
///
/// * control characters (Unicode `Cc`: NUL, C0, DEL, C1) — rejected;
/// * `:` anywhere (NTFS alternate-data-stream hazard) — rejected on every
///   platform for cross-platform consistency (XP-P-05);
/// * the remaining Windows-invalid set `< > " | ? *` (`/` and `\` are
///   handled structurally by the callers);
/// * Windows reserved device names via the shared stem-aware
///   `is_windows_reserved` (`CON`, `con.txt`, ... — XP-P-04);
/// * trailing `.` or trailing space (Windows strips both silently — XP-P-06;
///   rejected, never trimmed, so callers see exactly what was asked);
/// * component length over [`MAX_COMPONENT_BYTES`] (XP-P-07 partial).
///
/// `allow_drive_prefix` tolerates a leading `X:` (drive letter) in the
/// component: `require_safe_path_allow_absolute` accepts absolute Windows
/// paths like `C:\vault` (issue #590), whose drive colon is a separator, not
/// an ADS hazard. Flat filenames and relative-only paths pass `false` — a
/// filename can never be a drive, and a relative path must never contain one.
fn filename_component_error(component: &str, allow_drive_prefix: bool) -> Option<String> {
    if component.chars().any(char::is_control) {
        return Some("contiene caracteres de control no válidos".to_string());
    }
    let probe = if allow_drive_prefix && has_windows_drive_prefix(component) {
        &component[2..]
    } else {
        component
    };
    if probe.contains(':') {
        return Some("no debe contener ':' (riesgo de flujos alternativos NTFS)".to_string());
    }
    if component
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Some("contiene caracteres no permitidos en Windows: < > \" | ? *".to_string());
    }
    if is_windows_reserved(component) {
        return Some(
            "usa un nombre reservado de Windows (CON, PRN, AUX, NUL, COM1-9, LPT1-9)".to_string(),
        );
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Some("no debe terminar en '.' ni en espacio".to_string());
    }
    if component.len() > MAX_COMPONENT_BYTES {
        return Some(format!(
            "supera el límite de {MAX_COMPONENT_BYTES} bytes por componente"
        ));
    }
    None
}

/// Validate that `value` is a safe filesystem path: non-empty, ≤
/// [`MAX_PATH_LEN`], absolute paths ALLOWED (leading `/` accepted), and free
/// of `..` traversal components.
///
/// Unlike [`require_safe_path`], this variant accepts absolute paths so
/// tools like `detect_obsidian_vault` can accept `/home/user/vault`. Still
/// rejects `..` traversal and oversize inputs.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty, oversize, or
/// `..`-traversal paths — [`REASON_EMPTY`], [`REASON_TOO_LONG`], or
/// [`REASON_PATH_NOT_ALLOWED`] respectively.
pub fn require_safe_path_allow_absolute(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let path = Path::new(value);
    // Absolute paths are intentionally allowed — Obsidian vaults live at
    // user-supplied absolute locations (issue #590, bug #8).
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..' traversal components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Per-component filename hardening (issue #1608). The drive-prefix
    // exception keeps absolute Windows paths (`C:\vault`, #590) valid: the
    // drive colon is a separator there, not an ADS hazard.
    if let Some(reason) = normal_components(path).find_map(|c| filename_component_error(&c, true)) {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    Ok(path.to_path_buf())
}

/// Validate that `value` is a single, flat filename component safe to join
/// onto a base directory.
///
/// Unlike [`require_safe_name`] — which validates only length/emptiness and
/// assumes the value is never used as a path component — this enforces, by
/// structural decomposition, that the value cannot escape its parent directory
/// when joined via `Path::join`. This is the fix for issue #601: a `filename`
/// of `"../escape"` or `"sub/out"` must never reach `std::fs`.
///
/// Decomposition rules (Rust `Path::components`):
/// 1. Exactly **one** component.
/// 2. That component is of kind [`Component::Normal`] (rejects `ParentDir`
///    (`..`), `CurDir` (`.`), `RootDir` (`/`), and `Prefix` (Windows drive)).
/// 3. The component's string representation equals the original `value`
///    byte-for-byte, so platform-specific separator filtering (`/` and `\`)
///    cannot slip through.
///
/// Cross-platform filename hardening (issue #1608), via the private
/// `filename_component_error` helper with NO drive-prefix exception (a flat
/// filename can never be a drive): control characters (NUL included), `:`
/// anywhere (NTFS ADS hazard), the Windows-invalid set `< > " | ? *`,
/// Windows reserved device names (`CON`, `con.txt`, ... — stem-aware), and
/// trailing `.` / trailing space are all rejected — never trimmed, so
/// callers see exactly what was asked. Per-component cap: 255 bytes
/// (XP-P-07 partial; `\\?\` mitigation is out of scope).
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_TOO_LONG`], or [`REASON_PATH_NOT_ALLOWED`] if `value` is empty,
/// oversize, not a single flat `Normal` component, or violates any
/// filename-hardening rule.
pub fn require_safe_filename(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    // Reject separators explicitly so the rule is platform-independent: on
    // Unix `\` is a legal filename byte, but we must never let a
    // platform-specific component parse mask a real traversal risk (issue
    // #601). `Path::components` below is the structural backstop.
    if value.contains(['/', '\\']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no '/' or '\\' separators)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    let mut components = Path::new(value).components();
    let Some(component) = components.next() else {
        return Err(invalid_params_with_reason(
            field,
            "must be a single filename component",
            REASON_PATH_NOT_ALLOWED,
        ));
    };
    if components.next().is_some() {
        return Err(invalid_params_with_reason(
            field,
            "must not contain path separators or directory components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if !matches!(component, Component::Normal(_)) {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no '.', '..', '/', or drive prefix)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if component.as_os_str().to_string_lossy() != value {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no embedded separators)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Cross-platform hardening (issue #1608). `false`: a flat filename can
    // never carry a drive prefix, so every `:` is rejected.
    if let Some(reason) = filename_component_error(value, false) {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    Ok(())
}

/// A filename that is safe to join onto a base directory — guaranteed by
/// construction (issue #601).
///
/// A value of this type can ONLY be produced by [`SanitizedFilename::try_from`]
/// (or [`std::str::FromStr`]), which rejects anything that is not a single flat
/// [`Component::Normal`]. Handlers must thread this type across layers instead
/// of a raw `String`, so an unvalidated `..` can never reach `std::fs`.
///
/// This makes the "invalid state unrepresentable": the only way to obtain a
/// `SanitizedFilename` is through validation, and the validation is exhaustive
/// at the boundary. There is no `unsafe` escape hatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedFilename(String);

impl SanitizedFilename {
    /// Borrow the validated, flat filename.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SanitizedFilename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for SanitizedFilename {
    type Err = McpError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        require_safe_filename("filename", s).map(|()| SanitizedFilename(s.to_string()))
    }
}

impl TryFrom<&str> for SanitizedFilename {
    type Error = McpError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        std::str::FromStr::from_str(value)
    }
}
