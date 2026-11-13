use super::*;

/// Rejection of a request URL whose host is a forbidden IP literal (F-06 +
/// F-32, #1217). Carries the offending host and parsed address for typed UX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenLiteral {
    /// Host as it appeared in the request URL.
    pub host: String,
    /// Parsed forbidden address the host denotes.
    pub ip: IpAddr,
}

impl std::fmt::Display for ForbiddenLiteral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Spanish user-facing copy; the `SSRF detectado` prefix is the contract
        // the MCP probe suite asserts (`ssrf_probe_test.rs`).
        write!(
            f,
            "SSRF detectado: la IP {} del host '{}' está prohibida \
             (acceso a red interna/cloud metadata bloqueado)",
            self.ip, self.host
        )
    }
}

impl std::error::Error for ForbiddenLiteral {}
