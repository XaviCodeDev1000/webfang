//! The configuration-load error, split out of `cli::config` so that module
//! keeps its file-size ratchet headroom while the enum grows.
//!
//! Mechanical move: the variants and their user-facing messages are the ones
//! `cli::config` has always rendered, re-exported from `cli::config` so every
//! caller keeps the same path.

/// Why a configuration file could not be used (#1659).
///
/// `ConfigDefaults::load` degrades on
/// `NotFound` only; every other variant is reported. Messages are user-facing
/// and therefore in English, as are the `tracing` events that accompany them.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    /// `WEBFANG_TENANT_ID` was set to a value that cannot be one path
    /// component (S17). Rejected at load time so no run can silently resolve
    /// its paths against the single-tenant default instead.
    #[error("invalid tenant or run identifier (WEBFANG_TENANT_ID): {reason}")]
    InvalidTenantId {
        /// Why the identifier was rejected, in English.
        reason: String,
    },
    /// An explicit `WEBFANG_CONFIG` override was not an absolute path.
    #[error("WEBFANG_CONFIG must be an absolute path: {path}")]
    NotAbsolute {
        /// The rejected value, verbatim.
        path: String,
    },
    /// No file exists at the configured path.
    #[error("the configuration file does not exist: {path}")]
    NotFound {
        /// The path that was looked up.
        path: String,
    },
    /// The file exists but could not be read — `PermissionDenied`, a path
    /// that is a directory (`EISDIR`), invalid UTF-8, and friends.
    #[error("could not read the configuration file {path}: {source}")]
    Io {
        /// The path that was read.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The file was read but is not valid TOML.
    #[error("the configuration file {path} is not valid TOML: {source}")]
    Malformed {
        /// The path that was parsed.
        path: String,
        /// The underlying TOML failure.
        #[source]
        source: toml::de::Error,
    },
}
