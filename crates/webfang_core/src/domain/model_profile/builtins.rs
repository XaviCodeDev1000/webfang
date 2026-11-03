//! The repo's shipped Granite profiles, pinned to what this repo can verify.
//!
//! ADR-0004 §"Perfiles de modelo" is the authority for the numbers below.
//! Extracted from `model_profile.rs` (Ola 0, #2597); behavior unchanged.

use super::{ModelProfile, ProfileProvenance, PROFILE_SCHEMA_VERSION};

/// Id of the default 97M-parameter Granite embedding profile.
pub const GRANITE_97M_PROFILE_ID: &str = "granite-97m";

/// Id of the 311M-parameter Granite embedding profile (higher precision).
pub const GRANITE_311M_PROFILE_ID: &str = "granite-311m";

/// Repo authority that pins the native dimensions and the Matryoshka behaviour
/// of both Granite tiers.
///
/// ADR-0004 §"Perfiles de modelo" is the authority that survives the removal
/// of the local inference stack, and the recovery tag `onnx-tree/pre-step5`
/// (commit `38ab6a31`) is where the deleted literals live: `AiModel::embedding_dim`
/// returned `384` / `768` and `AiModel::output_dim` returned `384`.
///
/// Naming the tag is deliberate: after ADR-0004 paso 6 no file in this repo
/// defines those numbers any more, so a citation pointing at a deleted path
/// would be a dead link in the very output it exists to make auditable. The
/// symbol names travel with the tag on purpose — a citation that names the
/// symbol it pins cannot silently drift onto a neighbouring comment, which is
/// the one failure a provenance record cannot afford.
const CITATION_DIMENSIONS: &str =
    "docs/adr/0004-onnx-local-removal.md:87-89 (ADR-0004 §Perfiles de modelo); \
     tag `onnx-tree/pre-step5` (commit 38ab6a31): AiModel::embedding_dim (384/768), \
     AiModel::output_dim (384)";

/// Repo source that pins the 32 768-token sequence length
/// (`domain::options_spec::ai::DEFAULT_MAX_CHARS` = `32_768 * 3` at `:158`).
const CITATION_CONTEXT: &str =
    "crates/webfang_core/src/domain/options_spec/ai.rs:158 (DEFAULT_MAX_CHARS = 32_768 * 3)";

/// Repo source that pins the default characters-per-token ratio
/// (`domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN` at `:148`).
const CITATION_CHARS_PER_TOKEN: &str =
    "crates/webfang_core/src/domain/options_spec/ai.rs:148 (DEFAULT_CHARS_PER_TOKEN)";

/// Ids of the shipped profiles, in declaration order.
pub(super) const BUILTIN_IDS: [&str; 2] = [GRANITE_97M_PROFILE_ID, GRANITE_311M_PROFILE_ID];

/// The repo's shipped profiles, in declaration order.
///
/// Only models this repo can verify are here. `granite-97m` is 384d native
/// with no truncation; `granite-311m` is 768d native truncated to 384d via
/// Matryoshka, and both land 384d in the vault.
#[must_use]
pub fn builtin_profiles() -> Vec<ModelProfile> {
    let citation = format!("{CITATION_DIMENSIONS}; {CITATION_CONTEXT}; {CITATION_CHARS_PER_TOKEN}");
    vec![
        ModelProfile {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            schema_version: PROFILE_SCHEMA_VERSION,
            dim: 384,
            // No repo-verified per-request cap: the ONNX path never declared
            // one, and inventing a number for a remote runtime is the failure
            // mode this whole unit exists to prevent.
            batch_max: None,
            query_prefix: None,
            passage_prefix: None,
            context: 32_768,
            chars_per_token: crate::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            matryoshka: false,
            verified: true,
            provenance: ProfileProvenance::RepoVerified,
            citation: citation.clone(),
        },
        ModelProfile {
            id: GRANITE_311M_PROFILE_ID.to_string(),
            schema_version: PROFILE_SCHEMA_VERSION,
            dim: 768,
            batch_max: None,
            query_prefix: None,
            passage_prefix: None,
            context: 32_768,
            chars_per_token: crate::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            matryoshka: true,
            verified: true,
            provenance: ProfileProvenance::RepoVerified,
            citation,
        },
    ]
}
