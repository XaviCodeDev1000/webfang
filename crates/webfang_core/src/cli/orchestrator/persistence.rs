//! Persistence-root resolution for the crawl pipeline.
//!
//! Converges the Markdown output dir, asset dir and vault into one root
//! before handing them to the crawl/export engines (#638, #762).

use crate::application::crawl_options::CrawlOptions;
use crate::domain::persistence::PersistenceMode;
use tracing::warn;

/// Resolve the root directory that must contain the scraped Markdown AND the
/// downloaded assets.
///
/// When Obsidian `--quick-save` is active, both must share the vault as their
/// base so the vault stays self-contained (#638): if the `Downloader` keeps
/// using `output_dir` (`-o`) while the Markdown goes to the vault, relative
/// asset paths escape the vault and images stop rendering. The Downloader is a
/// slave of the config — the orchestrator is responsible for converging the
/// two persistence roots before handing them to the crawl/export engines.
///
/// An EXPLICIT `--vault` flag (captured as `vault_is_explicit` at parse time,
/// #762) extends the same invariant: the vault becomes the output base so
/// Markdown, assets and the RAG export all land inside it without the user
/// duplicating the path in `-o`. Vaults filled from `config.toml` or
/// autodetection do NOT redirect — that is why explicitness is tracked
/// separately from `obsidian_vault.is_some()`.
///
/// If `obsidian_vault` is somehow `None` at use time (should be unreachable
/// after preflight validation), fall back to `output_dir` — never panic.
pub(crate) fn resolve_persistence_root(opts: &CrawlOptions) -> std::path::PathBuf {
    if opts.export.quick_save || opts.export.vault_is_explicit {
        opts.export
            .obsidian_vault
            .clone()
            .unwrap_or_else(|| opts.export.output_dir.clone())
    } else {
        opts.export.output_dir.clone()
    }
}

/// Resolve the directory for the RAG pipeline export (`export.jsonl` /
/// `export.json`).
///
/// The persistence root ([`resolve_persistence_root`]) owns where Markdown
/// and assets are written; this helper exists so every sink of
/// `opts.export.output_dir` converges on ONE resolution path instead of each
/// call site re-deriving its own base (#762). The two diverge by design:
///
/// - `--quick-save` routes Markdown into `<persistence root>/_inbox` while
///   the RAG export keeps its historical `-o` destination (unchanged by
///   #762).
/// - Explicit `--vault` without `--quick-save` also redirects the RAG export
///   into the vault root, so a single `--vault` flag makes the vault
///   self-contained.
pub(super) fn resolve_export_dir(opts: &CrawlOptions) -> std::path::PathBuf {
    if opts.export.quick_save {
        opts.export.output_dir.clone()
    } else {
        resolve_persistence_root(opts)
    }
}

/// Resolve the persistence mode and warn about ignored CLI flags.
///
/// The domain resolver is pure (#1045): it never logs. `--state-dir`
/// that had no effect (resolved `Disabled` while a state dir was passed)
/// is reported via `ResolverNotes` and warned about here, the one call
/// site that knows about user flags.
pub(crate) fn resolve_persistence_mode(opts: &CrawlOptions) -> PersistenceMode {
    let default_state_dir = crate::cli::scrape_flow::resolve_default_state_dir();
    let (persistence_mode, resolver_notes) =
        crate::domain::persistence::PersistenceMode::from_config_with_notes(
            &opts.crawl.resume_config(),
            &default_state_dir,
        );
    if let Some(ignored_state_dir) = resolver_notes.ignored_state_dir {
        warn!(
        state_dir = ?ignored_state_dir,
        "ignoring --state-dir without --resume"
        );
    }
    persistence_mode
}
