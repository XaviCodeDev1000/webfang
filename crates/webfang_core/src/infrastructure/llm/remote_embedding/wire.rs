/// Wire request for OpenAI-compatible `POST /embeddings`.
///
/// `extra_body` is flattened at the BODY level, not nested: strict servers
/// (vLLM, NVIDIA NIM) reject a nested `extra_body` object with HTTP 400.
/// WebFang's own fields are declared LAST so they overwrite any same-named
/// key from the operator map (#3074).
#[derive(serde::Serialize)]
pub(super) struct EmbeddingRequest<'a> {
    #[serde(flatten)]
    pub(super) extra_body: &'a crate::domain::providers::ExtraBody,
    pub(super) model: &'a str,
    pub(super) input: &'a [String],
    /// `Some("float")` only when the provider opts in; omitted by default so
    /// the body stays byte-identical for providers that reject the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) encoding_format: Option<&'static str>,
}

/// Wire response: only the fields this adapter consumes.
///
/// Tolerancia deliberada (fixtures de contrato en
/// `fixtures/remote_embedding/`): vendors add `id`, `created`,
/// `system_fingerprint` and per-datum extras — all ignored. `usage` is
/// `Option` + `#[serde(default)]` (vendors may send `null` or omit it);
/// `index` is `Option` (order is positional 1:1, never re-sorted).
#[derive(serde::Deserialize)]
pub(super) struct EmbeddingResponse {
    pub(super) data: Vec<EmbeddingDatum>,
    #[serde(default)]
    pub(super) usage: Option<EmbeddingUsage>,
}

/// One datum of the wire response.
///
/// `index` is parsed for contract tolerance but never used for ordering:
/// vectors map 1:1 to inputs positionally (count-checked in
/// [`parse_embedding_body`]).
#[derive(serde::Deserialize)]
pub(super) struct EmbeddingDatum {
    #[allow(dead_code)]
    #[serde(default)]
    pub(super) index: Option<u32>,
    pub(super) embedding: Vec<f32>,
}

/// Token usage reported by the vendor (informational only).
#[derive(Debug, serde::Deserialize)]
pub(super) struct EmbeddingUsage {
    #[serde(default)]
    pub(super) prompt_tokens: u32,
    #[serde(default)]
    pub(super) total_tokens: u32,
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
