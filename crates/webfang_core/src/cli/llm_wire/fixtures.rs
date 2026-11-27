//! Shared arrange fixtures for the `llm_wire` tests — extracted verbatim from
//! the in-file `tests` mod (ADR-0021 Slice 1 headroom under the descending
//! file-size ratchet; pure move, no assertion changed).

use super::{Capability, CrawlOptions, ProviderConfig, ProviderKind};
use crate::domain::auth_source::AuthSource;

pub(super) fn opts_with_llm(extract: bool, provider_id: Option<&str>) -> CrawlOptions {
    CrawlOptions {
        extract_with_llm: extract,
        llm_provider: provider_id.map(str::to_string),
        ..Default::default()
    }
}

pub(super) fn provider(id: &str, kind: ProviderKind, auth: AuthSource) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        display_name: id.to_string(),
        kind,
        auth,
        capabilities: vec![Capability::Completion],
        model: Some("gpt-test".to_string()),
        ..crate::domain::providers::test_provider_config()
    }
}
