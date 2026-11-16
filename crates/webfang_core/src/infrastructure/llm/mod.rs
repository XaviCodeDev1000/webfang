//! LLM infrastructure — OpenAI-compatible chat/completions adapter (#789)
//! plus the opt-in remote embeddings adapter (#1462). The guarded HTTP
//! client constructor shared by both transports lives in [`http_client`]
//! (ADR-0020 Slice 2).

pub mod client;
pub mod http_client;
pub mod provider;
pub mod remote_embedding;
