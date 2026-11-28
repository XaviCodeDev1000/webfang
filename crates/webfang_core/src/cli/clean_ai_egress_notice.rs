//! The one human-facing notice for `--clean-ai` egress (T6, issue #3117 /
//! plan decision D1).
//!
//! Semantic cleaning sends PAGE TEXT to the configured embeddings endpoint.
//! D1 closed that question as a warning rather than a blocking consent: an
//! automations-breaking prompt would be worse than the disclosure it buys, so
//! the operator gets one line naming the endpoint HOST — never the full URL,
//! never a credential — and the run proceeds.
//!
//! # Why the host and nothing else
//!
//! The host answers the operator's only real question ("where does my text
//! go?"). The rest of the URL is deployment detail that can itself be
//! sensitive — an OpenAI-compatible gateway may embed a tenant or a token in
//! its path — and a log line is not the place to leak either.
//!
//! # Where it is emitted (contract, not an accident)
//!
//! [`emit_clean_ai_egress_notice`] is called from
//! [`crate::cli::llm_wire::build_embedding_provider_with_registry`] — the ONE
//! place a remote embeddings provider is resolved for the run — after the
//! `--offline` guard (so a run that will not dial announces nothing) and
//! before the credential is resolved and the endpoint probed (so the notice
//! precedes the first byte of page text that could leave the machine). That
//! call site is downstream of `init_logging_dual`, which is what makes the
//! event renderable at all (#796).
//!
//! Gated on `CrawlOptions::ai` (`--clean-ai`), not on provider resolution:
//! `--embedding-provider` alone wires the vault-search slot, which embeds the
//! operator's OWN notes and never scraped page text. A notice there would
//! describe an egress the run does not perform.

/// The phrase the notice exists to say, exported so tests assert on the same
/// source of truth the binary prints instead of on a copy of it.
///
/// Deliberately a whole clause: counting occurrences in captured stderr then
/// counts NOTICES, never the lines that happen to mention egress.
pub const CLEAN_AI_EGRESS_NOTICE_MARKER: &str = "page text leaves this machine";

/// Announce that semantic cleaning will send page text to `endpoint_host`.
///
/// Called once per `--clean-ai` run; silent otherwise. The prose travels as
/// the event message and the host as a structured field, so the compact
/// formatter never prints the same sentence twice (the shape the
/// orphan-threshold and SSRF-hatch notices follow).
pub fn emit_clean_ai_egress_notice(endpoint_host: &str) {
    // `target: "webfang"` pins the module path operators and snapshots key on:
    // this is the CLI's own notice, not an infrastructure log line, and moving
    // the function between child modules must not rename its source.
    tracing::warn!(
        target: "webfang",
        notice = "clean_ai_egress",
        endpoint_host = %endpoint_host,
        "semantic cleaning sends page text to the configured embeddings \
         endpoint: page text leaves this machine. The notice names the host \
         only; pass --offline to refuse every embeddings provider, or drop \
         --clean-ai to keep the raw extracted text."
    );
}

#[cfg(test)]
mod tests {
    //! The counting anchor must occur once INSIDE the text, or every
    //! behavioural count of "notices" would silently read as 2.

    use super::CLEAN_AI_EGRESS_NOTICE_MARKER;

    #[test]
    fn the_anchor_is_a_single_clause() {
        assert_eq!(
            CLEAN_AI_EGRESS_NOTICE_MARKER,
            "page text leaves this machine"
        );
        assert!(
            !CLEAN_AI_EGRESS_NOTICE_MARKER.contains('.'),
            "the anchor must be a clause, not a sentence boundary"
        );
    }
}
