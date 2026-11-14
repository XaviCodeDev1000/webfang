//! Executable resolution over PATH / PATHEXT (XP-S-04, #1608, #787).

use std::path::PathBuf;

/// Whether `binary` names an explicit path (absolute or relative) rather
/// than a bare executable name resolved from `PATH`.
pub(super) fn has_path_separator(binary: &str) -> bool {
    binary.contains('/') || binary.contains('\\')
}

/// Semicolon-separated extension list tried after the bare name on Windows
/// (XP-S-04, #1608): the cmd.exe default `PATHEXT` value.
#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Parse a `PATHEXT`-style semicolon-separated extension list into ordered,
/// normalized extensions. Pure — unit-tested without process env.
///
/// Preserves order, drops empty entries, lowercases, and ensures each entry
/// starts with a dot (cmd.exe accepts both `EXE` and `.EXE` spellings).
pub(super) fn parse_pathext(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            if ext.starts_with('.') {
                ext
            } else {
                format!(".{ext}")
            }
        })
        .collect()
}

/// The platform's `PATHEXT` value, when the platform uses one.
///
/// Windows: the `PATHEXT` env var (or the cmd.exe default when unset).
/// Everywhere else: `None` — a bare name is tried exactly as given.
#[cfg(windows)]
fn platform_pathext() -> Option<String> {
    Some(std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string()))
}

#[cfg(not(windows))]
fn platform_pathext() -> Option<String> {
    None
}

/// Candidate file names for a bare executable name, in resolution order.
///
/// The exact name first, then — on Windows only — the name with each
/// `PATHEXT` extension appended, in `PATHEXT` order (XP-S-04, #1608). Pure
/// and unit-testable on every platform: unix callers pass `None`.
pub(super) fn bare_name_candidates(name: &str, pathext: Option<&str>) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if let Some(exts) = pathext {
        names.extend(
            parse_pathext(exts)
                .into_iter()
                .map(|ext| format!("{name}{ext}")),
        );
    }
    names
}

/// First existing *file* among `dir/name` combinations, dir-major order
/// (every candidate name is tried within a directory before moving to the
/// next — the same order the OS uses). Pure given the inputs; the only
/// effect is the `is_file` probe, unit-tested on Linux.
pub(super) fn first_existing_in_dirs(
    mut dirs: impl Iterator<Item = PathBuf>,
    names: &[String],
) -> Option<PathBuf> {
    dirs.find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Scan `PATH` entries (in order) for a file named `name` — the same lookup
/// the OS performs for a bare executable name (#787).
///
/// Pure: takes the `PATH` value as input so tests control the search space
/// without touching the process-global environment. On Windows the bare
/// name is expanded with the platform's `PATHEXT` order (XP-S-04, #1608).
pub(super) fn resolve_executable_in_path(name: &str, path_value: &str) -> Option<PathBuf> {
    if name.is_empty() || has_path_separator(name) {
        return None;
    }
    let names = bare_name_candidates(name, platform_pathext().as_deref());
    first_existing_in_dirs(std::env::split_paths(path_value), &names)
}

/// Resolve the configured Obscura binary to an existing file.
///
/// A value with a path separator must exist as a file exactly as given; a
/// bare name must resolve through `PATH` (#787).
pub(super) fn resolve_obscura_binary(binary: &str, path_value: &str) -> Option<PathBuf> {
    if has_path_separator(binary) {
        PathBuf::from(binary)
            .is_file()
            .then(|| PathBuf::from(binary))
    } else {
        resolve_executable_in_path(binary, path_value)
    }
}
