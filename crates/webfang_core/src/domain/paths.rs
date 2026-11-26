//! Platform user-directory resolution — the ONE definition of where webfang
//! puts its config and cache files (XP-F-05, #1608).
//!
//! Four call sites used to hand-roll `~/.config` / `~/.cache` with raw
//! `XDG_*` + `$HOME` logic, which baked the Linux convention into every
//! platform. All sites now go through these helpers, backed by the `dirs`
//! crate:
//!
//! - **Linux**: XDG semantics, unchanged — `dirs` honors `$XDG_CONFIG_HOME`
//!   / `$XDG_CACHE_HOME` and falls back to `$HOME/.config` / `$HOME/.cache`.
//! - **Windows**: `%APPDATA%` (Roaming) for config, `%LOCALAPPDATA%` for
//!   cache.
//! - **macOS**: `~/Library/Application Support` for config, `~/Library/Caches`
//!   for cache.
//!
//! On top of the platform convention, an ABSOLUTE `$XDG_CONFIG_HOME` /
//! `$XDG_CACHE_HOME` wins on every platform (dirs only honors them on
//! Linux). This is deliberate, not Linux leakage: the repo's hermeticity
//! contract (#1126) and several tests inject these vars to redirect webfang's
//! writes to a temp dir, and silently ignoring the injection on macOS/Windows
//! would make tests read and write the operator's real profile directories
//! (evidenced by `test_fresh_cache_served_from_disk_off_the_executor`
//! failing on both non-Linux CI lanes). The absolute-path requirement keeps
//! the same strictness `dirs` applies on Linux.
//!
//! Callers append the webfang-specific component (`webfang/…`) themselves so
//! the base-dir policy and the file-layout policy stay separable.

use std::path::PathBuf;

/// Base directory for user configuration files.
///
/// `None` means the platform could not produce a user home (e.g. no `$HOME`
/// and no `%APPDATA%`): callers pick their own fail-soft fallback, which is
/// the behavior each site already had.
#[must_use]
pub(crate) fn config_base_dir() -> Option<PathBuf> {
    xdg_override("XDG_CONFIG_HOME").or_else(dirs::config_dir)
}

/// Base directory for cache / state files.
///
/// `None` means the platform could not produce a user home: callers pick
/// their own fail-soft fallback, matching their previous behavior.
#[must_use]
pub(crate) fn cache_base_dir() -> Option<PathBuf> {
    xdg_override("XDG_CACHE_HOME").or_else(dirs::cache_dir)
}

/// Explicit XDG override, honored on every platform when ABSOLUTE (matching
/// the strictness `dirs` applies on Linux). Empty or relative values are
/// ignored.
fn xdg_override(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

// ============================================================================
// File-layout defaults (tenant-scoped when an identifier is set)
// ============================================================================

/// Default SQLite database directory name (home-relative).
pub const DEFAULT_DB_DIR: &str = ".webfang";

/// Default SQLite database file name.
pub(crate) const DEFAULT_DB_FILE: &str = "crawl.db";

// ============================================================================
// Tenant-or-run scope (S17)
// ============================================================================

/// Environment variable naming the tenant-or-run scope.
///
/// Unset or empty keeps the single-tenant layout (the historical home-relative
/// defaults). A value is validated once, at configuration load, so a typo can
/// never silently land one tenant's state in the shared default root.
pub(crate) const TENANT_ID_ENV: &str = "WEBFANG_TENANT_ID";

/// Maximum accepted identifier length, in bytes.
const TENANT_ID_MAX_BYTES: usize = 128;

/// A validated tenant-or-run identifier: one path component, `[A-Za-z0-9._-]`.
///
/// Separators, empty names and `.`/`..` are rejected instead of sanitized: a
/// lossy rewrite would let two distinct tenants collide on one directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TenantId(String);

impl TenantId {
    /// Validate a raw identifier.
    ///
    /// # Errors
    ///
    /// Returns [`TenantIdError`] when the value is empty, too long, a reserved
    /// path component, or contains anything outside the accepted alphabet.
    pub(crate) fn parse(raw: &str) -> Result<Self, TenantIdError> {
        if raw.is_empty() {
            return Err(TenantIdError::Empty);
        }
        if raw.len() > TENANT_ID_MAX_BYTES {
            return Err(TenantIdError::TooLong {
                max: TENANT_ID_MAX_BYTES,
            });
        }
        if raw == "." || raw == ".." {
            return Err(TenantIdError::ReservedComponent);
        }
        if let Some(bad) = raw
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        {
            return Err(TenantIdError::UnsupportedCharacter(bad));
        }
        Ok(Self(raw.to_string()))
    }

    /// The identifier as a path component.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a tenant-or-run identifier was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TenantIdError {
    /// The value was empty (and therefore indistinguishable from unset).
    #[error("the identifier must not be empty")]
    Empty,
    /// The value cannot be a single path component of bounded length.
    #[error("the identifier is longer than {max} bytes")]
    TooLong {
        /// The accepted ceiling, in bytes.
        max: usize,
    },
    /// The value contains a character outside the accepted alphabet.
    #[error("the identifier contains an unsupported character: {0:?}")]
    UnsupportedCharacter(char),
    /// The value is a relative path component, never a tenant name.
    #[error("the identifier must not be a path component such as '.' or '..'")]
    ReservedComponent,
}

/// The tenant-or-run scope on-disk state is keyed by.
///
/// `unscoped` keeps every historical default byte-identical; an identifier
/// moves config, identity, credentials, database, state, Chrome profile and
/// downloads under ONE root (`<config base>/webfang/tenants/<id>`). Explicit
/// CLI and environment overrides still win over every accessor here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TenantScope {
    identifier: Option<TenantId>,
}

impl TenantScope {
    /// The single-tenant scope: no identifier, historical defaults.
    #[must_use]
    pub(crate) const fn unscoped() -> Self {
        Self { identifier: None }
    }

    /// Build a scope from an explicit identifier (`None` = unscoped).
    ///
    /// # Errors
    ///
    /// Propagates [`TenantIdError`] for a non-empty invalid identifier.
    pub(crate) fn from_identifier(raw: Option<&str>) -> Result<Self, TenantIdError> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            Some(raw) => Ok(Self {
                identifier: Some(TenantId::parse(raw)?),
            }),
            None => Ok(Self::unscoped()),
        }
    }

    /// Read the scope from [`TENANT_ID_ENV`].
    ///
    /// # Errors
    ///
    /// Propagates [`TenantIdError`] for a set-but-invalid identifier.
    pub(crate) fn from_env() -> Result<Self, TenantIdError> {
        let raw = std::env::var(TENANT_ID_ENV).ok();
        Self::from_identifier(raw.as_deref())
    }

    /// Best-effort ambient scope for path-default call sites.
    ///
    /// Validation is fail-closed where it can be reported: the CLI's config
    /// load rejects an invalid identifier before any path is used, so this
    /// lenient read only decides the shape of a default in a process that
    /// never loaded configuration. An invalid value is logged and ignored
    /// rather than guessed at.
    #[must_use]
    pub(crate) fn current() -> Self {
        match Self::from_env() {
            Ok(scope) => scope,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    env = TENANT_ID_ENV,
                    "invalid tenant or run identifier: keeping the single-tenant defaults"
                );
                Self::unscoped()
            },
        }
    }

    /// Scoped root: `<config base>/webfang/tenants/<id>`.
    #[must_use]
    pub(crate) fn root(&self) -> Option<PathBuf> {
        self.identifier.as_ref().map(|id| {
            config_base_dir()
                .unwrap_or_else(|| PathBuf::from(".config"))
                .join("webfang")
                .join("tenants")
                .join(id.as_str())
        })
    }

    fn under_root(&self, leaf: &str) -> Option<PathBuf> {
        self.root().map(|root| root.join(leaf))
    }

    /// Scoped configuration file (`<root>/config.toml`).
    #[must_use]
    pub(crate) fn config_path(&self) -> Option<PathBuf> {
        self.under_root("config.toml")
    }

    /// Scoped `age` identity (`<root>/identity.key`).
    #[must_use]
    pub(crate) fn identity_path(&self) -> Option<PathBuf> {
        self.under_root("identity.key")
    }

    /// Scoped encrypted credential store (`<root>/credentials.age`).
    #[must_use]
    pub(crate) fn credentials_path(&self) -> Option<PathBuf> {
        self.under_root("credentials.age")
    }

    /// Scoped SQLite database (`<root>/crawl.db`).
    #[must_use]
    pub(crate) fn db_path(&self) -> Option<PathBuf> {
        self.under_root("crawl.db")
    }

    /// Scoped checkpoint/resume state directory (`<root>/state`).
    #[must_use]
    pub(crate) fn state_dir(&self) -> Option<PathBuf> {
        self.under_root("state")
    }

    /// Scoped Chrome profile directory (`<root>/chrome-profile`).
    ///
    /// Only the `chromium` feature launches a browser, so a default-feature
    /// build never calls this even though the path policy is unconditional.
    /// `allow` (not `expect`) because the test build does call it, and an
    /// unfulfilled expectation is itself a warning.
    #[cfg_attr(not(feature = "chromium"), allow(dead_code))]
    #[must_use]
    pub(crate) fn chrome_profile_dir(&self) -> Option<PathBuf> {
        self.under_root("chrome-profile")
    }

    /// Scoped default download directory (`<root>/downloads`).
    #[must_use]
    pub(crate) fn downloads_dir(&self) -> Option<PathBuf> {
        self.under_root("downloads")
    }
}

// ============================================================================
// Default paths — ONE definition per file, scoped or platform-relative
// ============================================================================

/// Platform-default configuration file.
///
/// Tenant-scoped when an identifier is set; otherwise
/// `<config base>/webfang/config.toml`.
#[must_use]
pub(crate) fn default_config_path() -> PathBuf {
    TenantScope::current().config_path().unwrap_or_else(|| {
        config_base_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("webfang")
            .join("config.toml")
    })
}

/// Platform-default SQLite database path.
///
/// Tenant-scoped when an identifier is set; otherwise
/// `~/.webfang/crawl.db`, falling back to `./crawl.db` without a home.
#[must_use]
pub fn default_db_path() -> PathBuf {
    TenantScope::current()
        .db_path()
        .unwrap_or_else(|| match dirs::home_dir() {
            Some(home) => home.join(DEFAULT_DB_DIR).join(DEFAULT_DB_FILE),
            None => PathBuf::from(DEFAULT_DB_FILE),
        })
}

/// Platform-default `age` identity file.
///
/// Tenant-scoped when an identifier is set; otherwise
/// `<config base>/webfang/identity.key`.
#[must_use]
pub(crate) fn default_identity_path() -> PathBuf {
    TenantScope::current().identity_path().unwrap_or_else(|| {
        config_base_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("webfang")
            .join("identity.key")
    })
}

/// Platform-default encrypted credential store.
///
/// Tenant-scoped when an identifier is set; otherwise
/// `<config base>/webfang/credentials.age`.
#[must_use]
pub(crate) fn default_credentials_path() -> PathBuf {
    TenantScope::current()
        .credentials_path()
        .unwrap_or_else(|| {
            config_base_dir()
                .unwrap_or_else(|| PathBuf::from(".config"))
                .join("webfang")
                .join("credentials.age")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the helpers must honor XDG env vars exactly like the
    /// hand-rolled code they replaced — this pins the byte-identical
    /// Linux contract of XP-F-05.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_config_base_dir_honors_xdg_config_home() {
        use webfang_test_utils::EnvGuard;

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", tmp.path().to_str().expect("utf8 tmp"))]);
        assert_eq!(config_base_dir(), Some(tmp.path().to_path_buf()));
        drop(guard);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_cache_base_dir_honors_xdg_cache_home() {
        use webfang_test_utils::EnvGuard;

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let guard = EnvGuard::with(&[("XDG_CACHE_HOME", tmp.path().to_str().expect("utf8 tmp"))]);
        assert_eq!(cache_base_dir(), Some(tmp.path().to_path_buf()));
        drop(guard);
    }

    /// A missing XDG var must not produce a crash or an empty path: the
    /// fallback chain (home-relative) still yields something usable.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_base_dirs_fall_back_to_home_relative_paths() {
        use webfang_test_utils::EnvGuard;

        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", ""), ("XDG_CACHE_HOME", "")]);
        // In a sandbox without $HOME this is None; with $HOME it is
        // $HOME/.config — both are the documented, non-panicking outcomes.
        if std::env::var_os("HOME").is_some() {
            assert!(config_base_dir().is_some());
            assert!(cache_base_dir().is_some());
        }
        drop(guard);
    }

    /// S17: two identifiers must never resolve to the same on-disk state.
    /// Each scoped surface is checked because a single shared leaf (say, one
    /// hardcoded `crawl.db` under the config base) would silently mix tenants
    /// even while the root itself differs.
    #[cfg(target_os = "linux")]
    #[test]
    fn two_identifiers_produce_disjoint_paths() {
        use webfang_test_utils::EnvGuard;

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", tmp.path().to_str().expect("utf8 tmp"))]);

        let tenant_a = TenantScope::from_identifier(Some("tenant-a")).expect("valid id");
        let tenant_b = TenantScope::from_identifier(Some("tenant-b")).expect("valid id");

        assert_ne!(tenant_a.root(), tenant_b.root());
        assert_ne!(tenant_a.db_path(), tenant_b.db_path());
        assert_ne!(tenant_a.config_path(), tenant_b.config_path());
        assert_ne!(tenant_a.identity_path(), tenant_b.identity_path());
        assert_ne!(tenant_a.credentials_path(), tenant_b.credentials_path());
        assert_ne!(tenant_a.state_dir(), tenant_b.state_dir());
        assert_ne!(tenant_a.chrome_profile_dir(), tenant_b.chrome_profile_dir());
        assert_ne!(tenant_a.downloads_dir(), tenant_b.downloads_dir());

        let root = tenant_a.root().expect("scoped root");
        for leaf in [
            tenant_a.db_path(),
            tenant_a.config_path(),
            tenant_a.identity_path(),
            tenant_a.credentials_path(),
            tenant_a.state_dir(),
            tenant_a.chrome_profile_dir(),
            tenant_a.downloads_dir(),
        ] {
            let leaf = leaf.expect("scoped path");
            assert!(
                leaf.starts_with(&root),
                "{leaf:?} must live under the scoped root {root:?}"
            );
        }
        assert!(
            tenant_a.db_path().expect("db path").starts_with(tmp.path()),
            "the scoped root must follow the platform config base"
        );
        drop(guard);
    }

    /// No identifier keeps every historical default: the scope resolves to
    /// nothing and each caller falls back to its own home-relative path.
    #[test]
    fn unscoped_tenant_resolves_no_paths() {
        let scope = TenantScope::from_identifier(None).expect("no identifier is valid");
        assert!(scope.root().is_none());
        assert!(scope.db_path().is_none());
        assert!(scope.config_path().is_none());
        assert!(scope.credentials_path().is_none());
        assert!(scope.state_dir().is_none());
        assert!(scope.chrome_profile_dir().is_none());
        assert!(scope.downloads_dir().is_none());
    }

    /// Traversal, separators and reserved components are rejected instead of
    /// sanitized: two distinct raw names must never collapse to one directory.
    #[test]
    fn invalid_tenant_identifiers_are_rejected() {
        assert_eq!(TenantId::parse(""), Err(TenantIdError::Empty));
        assert_eq!(TenantId::parse("."), Err(TenantIdError::ReservedComponent));
        assert_eq!(TenantId::parse(".."), Err(TenantIdError::ReservedComponent));
        assert_eq!(
            TenantId::parse("../escape"),
            Err(TenantIdError::UnsupportedCharacter('/'))
        );
        assert_eq!(
            TenantId::parse("a\\b"),
            Err(TenantIdError::UnsupportedCharacter('\\'))
        );
        assert_eq!(
            TenantId::parse("tenant a"),
            Err(TenantIdError::UnsupportedCharacter(' '))
        );
        assert_eq!(
            TenantId::parse(&"x".repeat(TENANT_ID_MAX_BYTES + 1)),
            Err(TenantIdError::TooLong {
                max: TENANT_ID_MAX_BYTES
            })
        );
        assert!(TenantId::parse("run-2026_11.26").is_ok());
    }

    /// An empty or whitespace-only environment value is unset, not invalid:
    /// a script that exports the variable before giving it a value keeps the
    /// single-tenant layout instead of failing the run.
    #[test]
    fn blank_identifier_is_unscoped() {
        assert_eq!(
            TenantScope::from_identifier(Some("  ")).expect("blank is unset"),
            TenantScope::unscoped()
        );
    }

    /// The XDG override must be ABSOLUTE to be honored (the same strictness
    /// `dirs` applies on Linux): a relative value must fall through to the
    /// platform convention instead of being taken verbatim. This is the
    /// contract the hermeticity harness (#1126) and the non-Linux CI lanes
    /// rely on.
    #[test]
    fn relative_xdg_override_is_ignored() {
        use webfang_test_utils::EnvGuard;

        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", "relative/config")]);
        let base = config_base_dir();
        assert_ne!(
            base.as_deref(),
            Some(std::path::Path::new("relative/config")),
            "a relative XDG override must never be taken verbatim"
        );
        drop(guard);
    }
}
