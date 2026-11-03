//! Obsidian registry (`obsidian.json`) reading and resolution.
//!
//! Reads the per-platform registry, picks the most recently opened vault
//! and verifies it still exists on disk. Extracted from `vault_detector.rs`
//! (Ola 0, #2597); behavior unchanged.

use std::path::{Path, PathBuf};

use super::is_valid_vault;

/// Get the Obsidian registry path for the current platform.
///
/// Returns:
/// - Linux: `~/.config/obsidian/obsidian.json`
/// - macOS: `~/Library/Application Support/obsidian/obsidian.json`
/// - Windows: `%APPDATA%\Obsidian\obsidian.json`
pub(super) fn get_registry_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let config = dirs::config_dir()?;
        Some(config.join("obsidian").join("obsidian.json"))
    }

    #[cfg(target_os = "macos")]
    {
        let app_support = dirs::data_dir()?;
        Some(app_support.join("obsidian").join("obsidian.json"))
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(|appdata| {
            PathBuf::from(appdata)
                .join("Obsidian")
                .join("obsidian.json")
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Read the Obsidian registry and return the most recently opened vault.
///
/// The registry contains a map of vault IDs to vault metadata. Returns the vault
/// with the most recent `ts` timestamp (last opened).
///
/// Reads `registry_path` verbatim when injected (`Some`) — used for hermetic
/// tests — or falls back to the per-platform location from
/// [`get_registry_path`] when `None`.
pub(super) fn get_vault_from_registry(registry_path: Option<&Path>) -> Option<PathBuf> {
    let registry_path = match registry_path {
        Some(path) => path.to_path_buf(),
        None => get_registry_path()?,
    };

    if !registry_path.is_file() {
        return missing_registry(&registry_path);
    }

    // Read and parse registry
    let content = read_registry(&registry_path)?;
    let json = parse_registry(&content)?;

    // Extract vaults object
    let vaults = json.get("vaults")?.as_object()?;

    // Find vault with highest timestamp (most recent)
    let (_id, vault_data, ts) = find_most_recent_vault(vaults)?;
    let path = vault_data.get("path")?.as_str()?;

    resolve_registry_vault(ts, path, &PathBuf::from(path))
}

/// Report a missing registry file and return `None`.
pub(super) fn missing_registry(registry_path: &Path) -> Option<PathBuf> {
    tracing::trace!("Obsidian registry not found: {}", registry_path.display());
    None
}

/// Verify the resolved registry vault still exists, or report it as stale.
pub(super) fn resolve_registry_vault(ts: i64, path: &str, vault_path: &Path) -> Option<PathBuf> {
    tracing::debug!("Found vault from registry (ts={}): {}", ts, path);

    if is_valid_vault(vault_path) {
        Some(vault_path.to_path_buf())
    } else {
        tracing::warn!("Registry vault no longer exists: {}", path);
        None
    }
}

/// Read the Obsidian registry file contents.
pub(super) fn read_registry(registry_path: &Path) -> Option<String> {
    match std::fs::read_to_string(registry_path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("Failed to read Obsidian registry: {}", e);
            None
        },
    }
}

/// Parse the registry JSON.
pub(super) fn parse_registry(content: &str) -> Option<serde_json::Value> {
    match serde_json::from_str(content) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("Failed to parse Obsidian registry: {}", e);
            None
        },
    }
}

/// Find the vault entry with the most recent `ts` timestamp (last opened).
pub(super) fn find_most_recent_vault(
    vaults: &serde_json::Map<String, serde_json::Value>,
) -> Option<(&str, &serde_json::Value, i64)> {
    let mut best_vault: Option<(&str, &serde_json::Value, i64)> = None;

    for (id, vault_data) in vaults {
        let _vault_path = vault_data.get("path")?.as_str()?;
        let ts = vault_data.get("ts")?.as_i64().unwrap_or(0);

        // Check if this vault is currently open (optional) and more recent
        let is_better = match best_vault {
            Some((_, _, best_ts)) => ts > best_ts,
            None => true,
        };

        if is_better {
            best_vault = Some((id, vault_data, ts));
        }
    }

    best_vault
}
