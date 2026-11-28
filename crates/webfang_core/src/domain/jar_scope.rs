//! Cookie jar scope — how long a cookie jar lives.
//!
//! The scope decides which requests share cookies, and therefore how far a
//! cookie obtained from one response can travel:
//!
//! - [`JarScope::Shared`] — one jar for the whole process. This is the
//!   historical behavior: a cookie set by any response is offered to every
//!   later request. It is the default so that adding the option changes
//!   nothing observable.
//! - [`JarScope::Operation`] — a fresh jar per operation. A cookie set while
//!   serving one operation is offered to the remaining hops of that same
//!   operation (redirects, retries, follow-up requests) and to nothing else.
//! - [`JarScope::Session`] — one jar per caller session, keyed by the MCP
//!   `Mcp-Session-Id` header. Cookies set while serving one session are offered
//!   to every later request of THAT session and to no other, so two agents
//!   driving the same server never see each other's cookies and one agent's
//!   login survives between its own calls.
//!
//! The scope is server-level configuration, never a per-request parameter.

use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// How long a cookie jar lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum JarScope {
    /// One jar for the whole process: cookies survive across operations.
    #[default]
    Shared,
    /// One fresh jar per operation: cookies never survive past the operation.
    Operation,
    /// One jar per caller session: cookies survive across that session's calls.
    Session,
}

impl JarScope {
    /// Whether this scope keeps a single jar for the whole process.
    #[must_use]
    pub fn is_shared(self) -> bool {
        matches!(self, Self::Shared)
    }

    /// Whether this scope keeps one jar per caller session.
    #[must_use]
    pub fn is_session(self) -> bool {
        matches!(self, Self::Session)
    }
}

impl fmt::Display for JarScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared => write!(f, "shared"),
            Self::Operation => write!(f, "operation"),
            Self::Session => write!(f, "session"),
        }
    }
}

impl std::str::FromStr for JarScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "shared" => Ok(Self::Shared),
            "operation" => Ok(Self::Operation),
            "session" => Ok(Self::Session),
            other => Err(format!(
                "unknown cookie jar scope '{other}': expected shared, operation or session"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scope_is_shared() {
        assert_eq!(JarScope::default(), JarScope::Shared);
        assert!(JarScope::default().is_shared());
    }

    #[test]
    fn only_shared_is_shared_and_only_session_is_session() {
        assert!(!JarScope::Operation.is_shared());
        assert!(!JarScope::Session.is_shared());
        assert!(JarScope::Session.is_session());
        assert!(!JarScope::Operation.is_session());
        assert!(!JarScope::Shared.is_session());
    }

    #[test]
    fn display_names_match_the_cli_values() {
        assert_eq!(JarScope::Shared.to_string(), "shared");
        assert_eq!(JarScope::Operation.to_string(), "operation");
        assert_eq!(JarScope::Session.to_string(), "session");
    }

    #[test]
    fn from_str_accepts_the_declared_values_case_insensitively() {
        assert_eq!("shared".parse::<JarScope>(), Ok(JarScope::Shared));
        assert_eq!("Operation".parse::<JarScope>(), Ok(JarScope::Operation));
        assert_eq!("SHARED".parse::<JarScope>(), Ok(JarScope::Shared));
        assert_eq!("session".parse::<JarScope>(), Ok(JarScope::Session));
        assert_eq!("SeSsIoN".parse::<JarScope>(), Ok(JarScope::Session));
    }

    #[test]
    fn from_str_rejects_unknown_values_and_names_the_valid_ones() {
        let err = "cookie".parse::<JarScope>().unwrap_err();
        assert!(err.contains("unknown cookie jar scope 'cookie'"), "{err}");
        assert!(
            err.contains("expected shared, operation or session"),
            "{err}"
        );
        assert!("".parse::<JarScope>().is_err());
    }

    #[test]
    fn serde_roundtrip_uses_kebab_case() {
        for scope in [JarScope::Shared, JarScope::Operation, JarScope::Session] {
            let json = serde_json::to_string(&scope).expect("serialize");
            let back: JarScope = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(scope, back);
        }
        assert_eq!(
            serde_json::to_string(&JarScope::Operation).expect("serialize"),
            r#""operation""#
        );
        assert_eq!(
            serde_json::to_string(&JarScope::Session).expect("serialize"),
            r#""session""#
        );
    }
}
