//! Scraping implementations
//!
//! Contains the actual scraping logic:
//! - Readability algorithm wrapper
//!
//! DOM pre-pruning, author extraction and fallback text extraction live in
//! `domain::html_extraction` (ADR-0012 sub-slice 3.D; the fallback duplicate
//! collapsed onto the domain helper in #2360).
//!
//! Selector repair lives here on both tiers: Tier 1 is lexical
//! (`dom_inspector::DefaultDomInspector`), Tier 2 is embedding-backed
//! (`embedding_semantic_inspector::EmbeddingSemanticInspector`, ADR-0004).

pub mod dom_inspector;
pub mod embedding_semantic_inspector;
pub mod readability;
