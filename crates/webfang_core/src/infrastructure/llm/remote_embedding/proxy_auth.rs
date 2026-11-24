//! ADR-0021 proxy-auth diagnostics for the remote embedding adapter.
//!
//! A `407` is terminal (zero retries) and is a failure of the PROXY hop, not
//! of the destination. The diagnostic distinguishes a configured Basic-auth
//! credential from an anonymous proxy dial using only a boolean provenance
//! flag — the secret is never logged and never appears in the message.

use crate::error::ai_failure::remote_status;
use crate::error::SemanticError;

/// Build the English, credential-free 407 diagnostic.
#[must_use]
pub(super) fn proxy_auth_error(
    provider_id: &str,
    proxy_auth_configured: bool,
    status: u16,
    url: &str,
) -> SemanticError {
    if proxy_auth_configured {
        tracing::warn!(
            provider_id = %provider_id,
            status,
            proxy_auth_configured = true,
            "forward proxy rejected the configured proxy credential"
        );
        remote_status(
            format!(
                "provider '{provider_id}' proxy returned {status}: rejected the configured \
                 credential in proxy.auth (no retry)"
            ),
            status,
            url,
            None,
        )
    } else {
        tracing::warn!(
            provider_id = %provider_id,
            status,
            proxy_auth_configured = false,
            "forward proxy required authentication but none was configured"
        );
        remote_status(
            format!(
                "provider '{provider_id}' proxy returned {status}: requires authentication but no \
                 proxy.auth was declared (no retry)"
            ),
            status,
            url,
            None,
        )
    }
}
