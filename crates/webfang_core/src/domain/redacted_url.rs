//! Credential-free rendering of provider URLs for output sinks.
//!
//! The unit lives here as a sibling file of [`crate::domain::providers`] and is
//! re-exported there, so the historical `providers::RedactedUrl` paths keep
//! resolving for every caller.

use url::Url;

/// Fixed text rendered when userinfo cannot be removed. Never derived from the
/// input URL, so a failed redaction cannot fail open with a credential.
pub const REDACTED_URL_PLACEHOLDER: &str = "<redacted provider url>";

/// A URL rendered without username or password, safe for every output sink.
///
/// The original is cloned and edited: config load and request egress keep the
/// configured URL, only the rendered text loses userinfo. When the authority
/// cannot be edited (a cannot-be-a-base URL), rendering fails closed to
/// [`REDACTED_URL_PLACEHOLDER`] instead of printing credential-looking text.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedUrl {
    rendered: String,
}

impl RedactedUrl {
    /// Render `url` with username and password removed.
    #[must_use]
    pub fn from_url(url: &Url) -> Self {
        // Always attempt the edit, even when the URL reports no userinfo: a
        // cannot-be-a-base URL can hide credential-looking text in its opaque
        // path, and only the failed edit reveals that it cannot be sanitized.
        let mut redacted = url.clone();
        if redacted.set_username("").is_err() || redacted.set_password(None).is_err() {
            return Self::placeholder();
        }
        Self {
            rendered: redacted.to_string(),
        }
    }

    /// Parse a stringified URL and redact it; unparseable text fails closed.
    #[must_use]
    pub fn from_str_lossy(raw: &str) -> Self {
        match Url::parse(raw) {
            Ok(url) => Self::from_url(&url),
            Err(_) => Self::placeholder(),
        }
    }

    /// Render an optional URL for an output field.
    ///
    /// A missing URL keeps the fixed `unknown` literal; a present one is
    /// redacted, and an unparseable one fails closed to `unknown` too. This
    /// keeps every URL-valued tracing field credential-free.
    #[must_use]
    pub fn from_option_str(raw: Option<&str>) -> Self {
        match raw {
            Some(value) => Self::from_str_lossy(value),
            None => Self {
                rendered: "unknown".to_owned(),
            },
        }
    }

    /// The fixed placeholder used when a URL cannot be redacted.
    #[must_use]
    pub fn placeholder() -> Self {
        Self {
            rendered: REDACTED_URL_PLACEHOLDER.to_owned(),
        }
    }

    /// The rendered, credential-free text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.rendered
    }
}

impl std::fmt::Debug for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RedactedUrl").field(&self.rendered).finish()
    }
}

impl std::fmt::Display for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl serde::Serialize for RedactedUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.rendered)
    }
}

impl From<&Url> for RedactedUrl {
    fn from(url: &Url) -> Self {
        Self::from_url(url)
    }
}

/// serde hook for [`crate::domain::providers::ProviderConfig`]'s `base_url`.
pub fn serialize_redacted_url<S>(url: &Url, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serde::Serialize::serialize(&RedactedUrl::from_url(url), serializer)
}

impl std::fmt::Debug for crate::domain::providers::ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("kind", &self.kind)
            .field("base_url", &RedactedUrl::from_url(&self.base_url))
            .field("auth", &self.auth)
            .field("capabilities", &self.capabilities)
            .field("model", &self.model)
            .field("embedding_dim", &self.embedding_dim)
            .field("allow_loopback", &self.allow_loopback)
            .field("network_policy", &self.network_policy)
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

impl crate::domain::providers::ProviderConfig {
    /// The configured base URL with userinfo removed, for output sinks.
    #[must_use]
    pub fn redacted_base_url(&self) -> RedactedUrl {
        RedactedUrl::from_url(&self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_url_strips_userinfo_but_keeps_the_endpoint() {
        let url = Url::parse("https://user:s3cret@host:8443/v1?key=public").expect("fixture url");
        let redacted = RedactedUrl::from_url(&url);
        assert_eq!(redacted.as_str(), "https://host:8443/v1?key=public");
        assert_eq!(redacted.to_string(), "https://host:8443/v1?key=public");
        assert_eq!(
            serde_json::to_string(&redacted).expect("serialize"),
            "\"https://host:8443/v1?key=public\""
        );
        assert!(!format!("{redacted:?}").contains("s3cret"));
    }

    #[test]
    fn redacted_url_fails_closed_when_authority_cannot_be_edited() {
        let opaque = Url::parse("mailto:user:s3cret@host/v1").expect("fixture url");
        let redacted = RedactedUrl::from_url(&opaque);
        assert_eq!(redacted.as_str(), REDACTED_URL_PLACEHOLDER);
        assert!(!format!("{redacted:?} {redacted}").contains("s3cret"));
    }
}
