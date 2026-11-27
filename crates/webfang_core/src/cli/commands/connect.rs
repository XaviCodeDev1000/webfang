//! `webfang connect` — detect and validate a remote embedding endpoint, then
//! write its config so a later `webfang --embedding-provider <id>` (or the
//! vault-search path) resolves it with no further setup.
//!
//! Contract (issue #1933, ADR-0004 step 4):
//! - **Detect + validate**: build a real `RemoteEmbeddingAdapter` against the
//!   endpoint and run its startup `probe_dim`. The adapter POSTs a probe
//!   embedding and reads the served dimension; a pin that disagrees fails
//!   closed. This is the same preflight the scrape path runs, so a `connect`
//!   that succeeds is a `connect` whose provider will start.
//! - **Write**: splice a `[[providers]]` record into the user's config so the
//!   registry resolves it on the next run. The on-disk document is edited in
//!   place: an existing record with the same id is MERGED field by field (never
//!   replaced wholesale, never duplicated), and a new one is appended. The
//!   write is also safe against the two ways the config around it is
//!   destroyed: the temp file is created O_EXCL under a unique per-write name
//!   (a pre-planted symlink is refused, never truncated through), and the
//!   bytes are re-read after the splice so a config a concurrent `connect`
//!   rewrote meanwhile is refused instead of renamed over.
//! - **Never write config when the endpoint did not respond** — the error names
//!   what was missing (URL, model, DNS / connection / status / parse). A
//!   refusal for any other reason (a type-invalid config, an operator-owned
//!   record bound to a different endpoint, an uneditable `providers` shape)
//!   likewise leaves the file byte-for-byte identical.
//!
//! Auth is always `{"source":"none"}` for the connect flow: the subcommand does
//! not ask for credentials, and an anonymous endpoint is the only honest shape
//! a discovery tool can register. An endpoint that needs auth is declared by
//! hand afterwards, which keeps this side-effect minimal and auditable.
//!
//! ## Why a model is required
//!
//! `RemoteEmbeddingAdapter` refuses to build without a declared `model` (the
//! remote endpoint requires it on every request), so the probe cannot run
//! without one. `connect` therefore requires `--model`: guessing it would
//! register a provider that fails at first use, which is exactly the failure
//! mode this subcommand exists to prevent.

use std::fmt::Write as _;
use std::path::Path;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::cli::llm_wire;
use crate::cli::parse::parse_provider_id;
use crate::domain::auth_source::AuthSource;
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::providers::{Capability, ProviderConfig, ProviderKind, ProvidersConfig};

/// Run the connect flow for the endpoint `base_url` under the id `id`,
/// writing the resulting provider record to `path`.
///
/// Returns the exit the binary should use. On success the provider record is
/// merged into (or appended to) `path`; on any failure nothing is written and
/// the message names what failed.
pub async fn run(base_url: &url::Url, id: &str, model: &str, path: &Path) -> CliExit {
    let id = match parse_provider_id(id) {
        Ok(id) => id,
        Err(e) => return CliExit::UsageError(e),
    };

    let provider = ProviderConfig {
        id: id.clone(),
        display_name: display_for_id(&id),
        kind: ProviderKind::OpenAiCompatible,
        base_url: base_url.clone(),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        // The model is required: the adapter cannot build without it.
        model: Some(model.to_string()),
        // `connect` never locks a dimension: the startup probe adopts the
        // served width, and writing a pin here would turn a healthy endpoint
        // into a fail-closed mismatch the operator never asked for.
        embedding_dim: None,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        // `connect` writes a direct-egress record: proxy support is opt-in
        // per provider in config (ADR-0021), never synthesized here.
        proxy: None,
        // The wizard cannot know vendor-specific fields; it writes none and
        // leaves the operator to add them after connecting (#3074).
        extra_body: serde_json::Map::new(),
        query_extra_body: serde_json::Map::new(),
    };

    register(&provider, path).await
}

/// Validate `provider` against its endpoint, then write it to `path`.
///
/// Split from [`run`] so tests can drive the same validation/write path with a
/// provider whose `allow_loopback` is set (the guarded production client
/// rejects loopback; wiremock lives there).
async fn register(provider: &ProviderConfig, path: &Path) -> CliExit {
    if let Err(exit) = validate_endpoint(provider).await {
        return exit;
    }

    if let Err(e) = write_provider(path, provider) {
        return CliExit::ConfigError(format!(
            "no se pudo escribir la configuración en '{}': {e}",
            path.display()
        ));
    }

    println!(
        "Provider '{}' conectado y registrado en {}",
        provider.id,
        path.display()
    );
    CliExit::Success
}

/// Build the real adapter against `provider` and run its startup probe.
///
/// This is the one network hop the subcommand makes — and it is the SAME
/// preflight the scrape path runs (`llm_wire::build_embedding_provider`), so a
/// provider that validates here is one that will start there. A failure is
/// returned verbatim so the message names the missing piece (URL gate, model,
/// connection, status, parse).
async fn validate_endpoint(provider: &ProviderConfig) -> Result<(), CliExit> {
    // #S14: the SSRF refusal is decided BEFORE the adapter is built and stays
    // out of the `ConfigError → NetworkError` remap below. A forbidden target
    // is not a network failure: 69 tells automation to retry a request that
    // must never leave the machine.
    //
    // The verdict uses the SAME predicate the adapter's guard applies, with
    // the provider's own `allow_loopback` and `network_policy` — a self-hosted
    // loopback endpoint (`allow_loopback: true`) or an allowlisted LAN CIDR is
    // legitimate, and refusing it here would break providers that work today.
    if let Some(host) = provider.base_url.host_str() {
        if crate::domain::ssrf_guard::redirect_literal_blocked_with_policy(
            Some(host),
            provider.allow_loopback,
            &provider.network_policy,
        ) {
            return Err(CliExit::SsrfBlocked(format!(
                "provider '{}' refused by the SSRF guard: host '{host}' is a forbidden \
                 internal address (internal network/cloud metadata access blocked)",
                provider.id
            )));
        }
    }

    let providers = ProvidersConfig {
        providers: vec![provider.clone()],
    };
    // `resolve_embedding_config` picks the explicit slot; we then build the
    // adapter (which probes). Building with a fresh `CrawlOptions` whose
    // embedding slot names this provider keeps the resolution unambiguous even
    // when the config already holds other providers.
    let opts = CrawlOptions {
        embedding_provider: Some(provider.id.clone()),
        ..CrawlOptions::default()
    };

    match llm_wire::build_embedding_provider(&opts, &providers, false).await {
        Ok(Some(_conn)) => Ok(()),
        Ok(None) => Err(CliExit::ConfigError(format!(
            "el provider '{}' no se resolvió como un endpoint remoto de embeddings: \
             el endpoint respondió pero no es OpenAI-compatible",
            provider.id
        ))),
        // `build_embedding_provider` maps BOTH construction failures (bad
        // `--model`) and probe failures (connection, status, parse) onto
        // `CliExit::ConfigError`, with the real cause in the message. The exit
        // code is remapped to `NetworkError` (69) because a `connect` that
        // could not validate an endpoint is, from the operator's seat, an
        // endpoint that did not answer.
        //
        // The SSRF refusal is NOT in that set: the entry guard answers it above
        // (#S14), before the adapter exists, so it leaves with 70.
        //
        // Known limitation: a construction failure that is NOT a network
        // problem (an HTTP 401 for a wrong model) still lands on 69.
        // Distinguishing it needs `build_embedding_provider` to surface the
        // category separately; until then the message — not the exit code —
        // carries the cause.
        Err(CliExit::ConfigError(msg)) => Err(CliExit::NetworkError(msg)),
        Err(other) => Err(other),
    }
}

/// Derive a readable label from the id `connect` generated.
///
/// `connect-f82b` → `Connect F82b`; `acme-inc-99` → `Acme Inc 99`. Purely
/// cosmetic: the id is what the registry matches on, and the label can be
/// edited by hand afterwards without changing behaviour.
fn display_for_id(id: &str) -> String {
    id.split('-')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = String::new();
                    let _ = write!(s, "{}{}", first.to_ascii_uppercase(), chars.as_str());
                    s
                },
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splice the provider record for `provider.id` into `path`.
///
/// **Idempotent by id, by MERGE.** `ProvidersConfig::find` returns the FIRST
/// match, so an append-only write would let a stale record silently shadow the
/// newly connected endpoint forever. A matching `[[providers]]` entry is
/// therefore merged field by field rather than replaced wholesale: a plain
/// replacement would silently downgrade a working provider to the anonymous,
/// loopback-forbidden shape `connect` builds. The merge's exact split is
/// [`merge_provider`]; a record whose operator-owned fields belong to a
/// DIFFERENT endpoint is refused instead of merged.
///
/// **The file is edited, never re-serialized.** The user's config holds far
/// more than providers (`format`, `concurrency`, `vault_path`,
/// `[[model_profiles]]`…), and `ProvidersConfig` models exactly one field.
/// Parsing to that struct and writing the round-trip back to disk would
/// silently DESTROY every key the shape does not know about. Instead the
/// on-disk TOML document is parsed into an editable tree, only the matching
/// `[[providers]]` entry is touched (or one appended), and every other byte —
/// keys, tables, ordering, comments — survives verbatim.
///
/// **What is guaranteed about a successful write.** The `providers` array is
/// well-formed TOML and the whole file deserializes as the `ConfigDefaults`
/// document the run loads (that gate runs before anything is written, so
/// anything the next run could not load — a type-invalid provider record or a
/// mistyped key anywhere in the file — is refused rather than written
/// through); the matching entry was
/// merged or appended and no duplicate id was created; the document was
/// rendered from the parsed tree, which preserves every untouched key, table
/// and comment — including the comments attached to the merged entry's own
/// refreshed keys. The write is atomic (temp file + rename), preserves the
/// file's permissions, and follows a symlinked config to its target. It is
/// also exclusive in effect: the bytes on disk are re-read after the splice,
/// so a config another `connect` rewrote meanwhile is refused instead of
/// renamed over (see [`ensure_config_unchanged`]).
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidData`] — the existing file is not valid TOML,
/// does not match the full `ConfigDefaults` document the run loads, declares a
/// `providers` value
/// that is neither an array of tables nor a convertible inline array, or holds
/// a record whose operator-owned fields belong to a different endpoint. Every
/// one of these is checked before the atomic write, so the file is left
/// byte-for-byte identical. The same [`std::io::ErrorKind::InvalidData`] is
/// returned when the config changed on disk while this write held it — a
/// concurrent `connect` landed first, nothing is written, and the message
/// names the retry. [`std::io::Error`] otherwise — I/O failure creating
/// the parent directory, the temp file, or performing the rename.
fn write_provider(path: &Path, provider: &ProviderConfig) -> Result<(), std::io::Error> {
    // The `webfang/` parent directory does not exist on a first run — exactly
    // the case `connect` exists for. Create it before opening, or the write
    // fails AFTER the network probe already succeeded.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // The snapshot the splice is computed from, kept for the optimistic
    // concurrency check after the merge (`ensure_config_unchanged`).
    let mut original: Option<String> = None;
    let mut document = if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        // The `DocumentMut` parse below only proves the file is WELL-FORMED
        // TOML; it accepts content the run cannot resolve. The gate therefore
        // deserializes the SAME document the run loads — `ConfigDefaults` —
        // not just its `providers` slice: `ProvidersConfig` models exactly one
        // field, so a mistyped key anywhere else (`format = 42`, an invalid
        // `[[model_profiles.profiles]]` entry) passed the old gate, connect
        // printed success and wrote — and the next run degraded EVERYTHING to
        // defaults in silence through the documented `#[serde(default)]`
        // fallback at `cli/config.rs`. `ConfigDefaults` deliberately does NOT
        // deny unknown fields (an operator's unmodeled keys keep working, and
        // an empty file — a fresh install — parses), so the gate only refuses
        // what the run itself could not load. It runs before any write handle
        // is opened, so a refusal leaves the file byte-identical.
        toml::from_str::<crate::cli::config::ConfigDefaults>(&existing).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración en '{}' no coincide con el documento completo que \
                     el run carga (no se toca el archivo para no corromperlo): {e}",
                    path.display()
                ),
            )
        })?;
        let document = existing.parse::<toml_edit::DocumentMut>().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración existente en '{}' no es TOML válido: {e}",
                    path.display()
                ),
            )
        })?;
        original = Some(existing);
        document
    } else {
        toml_edit::DocumentMut::new()
    };

    upsert_provider(&mut document, provider)?;

    // Optimistic concurrency: the whole splice is a read-modify-write with no
    // lock on the config, so the bytes are re-read AFTER the merge and must
    // still be the ones the merge started from. A difference means a
    // concurrent `connect` landed first: renaming now would drop its record
    // while both printed success, so the write refuses and names the retry.
    commit_splice(path, &document, original.as_deref())
}

/// Persist the spliced document: refuse first if the file drifted, then write
/// atomically.
///
/// Extracted from [`write_provider`] so the wiring itself is testable: the
/// drift check and the write are two calls that must happen IN ORDER, and a
/// test that removes one while keeping the other green is exactly the
/// unfalsifiable coverage this function exists to prevent.
fn commit_splice(
    path: &Path,
    document: &toml_edit::DocumentMut,
    original: Option<&str>,
) -> Result<(), std::io::Error> {
    ensure_config_unchanged(path, original)?;
    write_atomically(path, &document.to_string())
}

/// Whether the bytes on disk no longer match the snapshot the splice was
/// computed from.
///
/// Byte-exact on purpose: a whitespace-only difference still means another
/// writer touched the document, and merging into a snapshot that is no
/// longer what is on disk is exactly the lost update this guards.
fn config_drifted(original: &str, current_on_disk: &str) -> bool {
    original != current_on_disk
}

/// Fail closed when the config changed on disk since `original` was read.
///
/// `write_provider` is a read-modify-write with no portable advisory lock
/// on the config: two concurrent `connect` runs carrying different
/// providers would both splice into the same snapshot and race to rename,
/// and the loser's provider would vanish while BOTH printed "conectado y
/// registrado". The conflict is therefore detected optimistically — the
/// bytes are re-read after the splice and must still be the ones the
/// splice started from. A difference (including a file that appeared or
/// disappeared between the read and the write) means another writer landed
/// first: this run refuses, writes nothing, and names the retry.
///
/// The check narrows the window to the microseconds between the re-read
/// and the rename; closing it completely needs a lock on the config, which
/// this write does not take.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidData`] when the bytes drifted, the file
/// vanished, or it became unreadable.
fn ensure_config_unchanged(path: &Path, original: Option<&str>) -> Result<(), std::io::Error> {
    let current = std::fs::read_to_string(path).ok();
    match (original, current) {
        (Some(original), Some(current)) if !config_drifted(original, &current) => Ok(()),
        // Absent when read, absent now: a fresh first connect.
        (None, None) => Ok(()),
        _ => Err(config_drift_error(path)),
    }
}

/// The refusal for "another writer changed the config while connect held it".
fn config_drift_error(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "la configuración '{}' cambió en disco mientras connect la escribía: \
             no se toca el archivo para no perder el registro del otro connect; \
             ejecutá connect de nuevo",
            path.display()
        ),
    )
}

mod atomic_write;
mod toml_merge;

use atomic_write::write_atomically;
use toml_merge::upsert_provider;

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests_atomic;

#[cfg(test)]
mod tests_flow;

#[cfg(test)]
mod tests_merge;
