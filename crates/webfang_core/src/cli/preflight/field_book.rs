//! Field book of the normalization pipeline: `ArgSources`, `FieldBook`,
//! `NormalizedConfig`. Moved verbatim from `preflight.rs` (M3 PR B, option a).
use super::*;
use crate::domain::budget::BudgetOverrides;

use super::normalization::stage_defaults;

// ============================================================================
// Normalization pipeline (stabilization-config-normalization, Phase 3 — D3/D4)
// Private FieldBook + rank-guarded stages. Legacy merge functions stay below
// untouched (deleted in Phase 5). New code is not yet wired at call sites.
// ============================================================================

/// Provenance map `arg_id → source` for the env/cli stages.
///
/// Only explicit sources (`Environment`, `Cli`) are recorded; absent ids and
/// `DefaultValue` are omitted so those stages simply never write. Phase 5 wires
/// this via `parse_args()` → `ArgSources::capture` (design D1). Tests drive it
/// directly via `set` for hermeticity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArgSources {
    map: BTreeMap<String, ConfigSource>,
}

impl ArgSources {
    /// Record `id` as provided by `source` (only `Environment` or `Cli` are
    /// meaningful; `Default`/`ConfigFile` inserted here are ignored by
    /// convention but not rejected).
    pub fn set(&mut self, id: &str, source: ConfigSource) {
        self.map.insert(id.to_string(), source);
    }

    /// Source for `id`, if it was explicitly provided via env or CLI.
    #[must_use]
    pub fn source_of(&self, id: &str) -> Option<ConfigSource> {
        self.map.get(id).copied()
    }

    /// Capture per-arg provenance from already-parsed matches (design D1).
    ///
    /// One conceptual parse pass: clap builds `ArgMatches` once; reading
    /// `value_source` per contested id is O(1) afterwards. Only
    /// `CommandLine` and `EnvVariable` are recorded.
    #[must_use]
    pub fn capture(matches: &clap::ArgMatches) -> Self {
        use clap::parser::ValueSource;
        const CONTESTED: &[&str] = &[
            "url",
            "selector",
            "delay_ms",
            "jitter_ms",
            "max_pages",
            "concurrency",
            "use_sitemap",
            "sitemap_url",
            "max_depth",
            "timeout_secs",
            "format",
            "export_format",
            "output",
            "obsidian_tags",
            "obsidian_wiki_links",
            "obsidian_relative_assets",
            "obsidian_rich_metadata",
            "vault",
            "quick_save",
            "ignore_waf",
            "rate_limit_burst",
        ];
        let present: std::collections::HashSet<&str> =
            matches.ids().map(|id| id.as_str()).collect();
        let mut map = BTreeMap::new();
        for &id in CONTESTED {
            if !present.contains(id) {
                continue;
            }
            match matches.value_source(id) {
                Some(ValueSource::CommandLine) => {
                    map.insert(id.to_string(), ConfigSource::Cli);
                },
                Some(ValueSource::EnvVariable) => {
                    map.insert(id.to_string(), ConfigSource::Environment);
                },
                _ => {},
            }
        }
        Self { map }
    }
}

/// Private slot board mirroring the contested surface (~39 fields via D3).
///
/// Each slot is a `ConfigValue<T>` carrying both the normalized value and its
/// `ConfigSource` so stages decide writes purely by rank. Kept private so
/// ordering invariants stay inside this module; `NormalizedConfig` is the pub
/// output.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub(crate) struct FieldBook {
    // discovery / crawl
    pub(super) max_pages: ConfigValue<usize>,
    pub(super) max_depth: ConfigValue<u8>,
    pub(super) sitemap_depth: ConfigValue<u8>,
    pub(super) sitemap_url: ConfigValue<Option<String>>,
    pub(super) use_sitemap: ConfigValue<bool>,
    pub(super) selector: ConfigValue<String>,
    // network / crawler
    pub(super) delay_ms: ConfigValue<u64>,
    pub(super) jitter_ms: ConfigValue<u64>,
    pub(super) timeout_secs: ConfigValue<u64>,
    pub(super) concurrency: ConfigValue<ConcurrencyConfig>,
    // output / export
    pub(super) output: ConfigValue<PathBuf>,
    pub(super) format: ConfigValue<OutputFormat>,
    pub(super) export_format: ConfigValue<ExportFormat>,
    // obsidian
    pub(super) obsidian_tags: ConfigValue<Vec<String>>,
    pub(super) obsidian_wiki_links: ConfigValue<bool>,
    pub(super) obsidian_relative_assets: ConfigValue<bool>,
    pub(super) obsidian_rich_metadata: ConfigValue<bool>,
    pub(super) vault: ConfigValue<Option<PathBuf>>,
    pub(super) quick_save: ConfigValue<bool>,
    // behavior
    pub(super) ignore_waf: ConfigValue<bool>,
    // budget model overrides (additive; Q1 burst knob — design D4)
    pub(super) budget_overrides: ConfigValue<BudgetOverrides>,
}

impl Default for FieldBook {
    fn default() -> Self {
        stage_defaults()
    }
}

/// Pipeline output — `ConfigValue<T>` slots + projection to downstream type.
///
/// `CrawlOptions` itself stays untouched (design D4); this is the one-directional
/// projection `NormalizedConfig::into_crawl_options` dropping provenance at the
/// boundary.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct NormalizedConfig {
    pub max_pages: ConfigValue<usize>,
    pub max_depth: ConfigValue<u8>,
    pub sitemap_depth: ConfigValue<u8>,
    pub sitemap_url: ConfigValue<Option<String>>,
    pub use_sitemap: ConfigValue<bool>,
    pub selector: ConfigValue<String>,
    pub delay_ms: ConfigValue<u64>,
    pub jitter_ms: ConfigValue<u64>,
    pub timeout_secs: ConfigValue<u64>,
    pub concurrency: ConfigValue<ConcurrencyConfig>,
    pub output: ConfigValue<PathBuf>,
    pub format: ConfigValue<OutputFormat>,
    pub export_format: ConfigValue<ExportFormat>,
    pub obsidian_tags: ConfigValue<Vec<String>>,
    pub obsidian_wiki_links: ConfigValue<bool>,
    pub obsidian_relative_assets: ConfigValue<bool>,
    pub obsidian_rich_metadata: ConfigValue<bool>,
    pub vault: ConfigValue<Option<PathBuf>>,
    pub quick_save: ConfigValue<bool>,
    pub ignore_waf: ConfigValue<bool>,
    /// Operator-level budget overrides carried to `BudgetModel::build`
    /// (design D4). Additive slot: the default (`rate_burst: None`)
    /// reproduces today's derived numbers exactly.
    pub budget_overrides: ConfigValue<BudgetOverrides>,
}

impl NormalizedConfig {
    pub(crate) fn from_book(book: FieldBook) -> Self {
        Self {
            max_pages: book.max_pages,
            max_depth: book.max_depth,
            sitemap_depth: book.sitemap_depth,
            sitemap_url: book.sitemap_url,
            use_sitemap: book.use_sitemap,
            selector: book.selector,
            delay_ms: book.delay_ms,
            jitter_ms: book.jitter_ms,
            timeout_secs: book.timeout_secs,
            concurrency: book.concurrency,
            output: book.output,
            format: book.format,
            export_format: book.export_format,
            obsidian_tags: book.obsidian_tags,
            obsidian_wiki_links: book.obsidian_wiki_links,
            obsidian_relative_assets: book.obsidian_relative_assets,
            obsidian_rich_metadata: book.obsidian_rich_metadata,
            vault: book.vault,
            quick_save: book.quick_save,
            ignore_waf: book.ignore_waf,
            budget_overrides: book.budget_overrides,
        }
    }

    /// Project the normalized configuration into the downstream engine type.
    ///
    /// Provenance is dropped here — merge decisions are done.
    #[must_use]
    pub fn into_crawl_options(self) -> CrawlOptions {
        let mut opts = CrawlOptions::default();
        opts.crawl.max_pages = self.max_pages.value;
        opts.crawl.max_depth = self.max_depth.value;
        opts.crawl.sitemap_url = self.sitemap_url.value;
        opts.crawl.use_sitemap = self.use_sitemap.value;
        opts.crawl.selector = self.selector.value;
        opts.network.delay_ms = self.delay_ms.value;
        opts.network.jitter_ms = self.jitter_ms.value;
        opts.network.timeout_secs = self.timeout_secs.value;
        // Explicit operator concurrency (CLI/TOML, not "auto") feeds
        // the budget model as a crawl-tier override — design D4
        // explicit-wins rule; provenance already rank-guarded upstream.
        // NOTE: read BEFORE the move of self.concurrency.value below.
        let explicit_crawl = self.concurrency.value.get();
        opts.network.concurrency = self.concurrency.value;
        opts.export.output_dir = self.output.value;
        opts.export.output_format = self.format.value;
        opts.export.export_format = self.export_format.value;
        opts.export.obsidian_tags = self.obsidian_tags.value;
        opts.export.obsidian_wiki_links = self.obsidian_wiki_links.value;
        opts.export.obsidian_relative_assets = self.obsidian_relative_assets.value;
        opts.export.obsidian_rich_metadata = self.obsidian_rich_metadata.value;
        opts.export.obsidian_vault = self.vault.value;
        opts.export.quick_save = self.quick_save.value;
        opts.crawl.ignore_waf = self.ignore_waf.value;
        opts.budget_overrides = self.budget_overrides.value;
        if let Some(explicit) = explicit_crawl {
            opts.budget_overrides.crawl =
                crate::domain::budget::tiers::CrawlConcurrency::new(explicit).ok();
        }
        // cross-field rule already applied in the pipeline
        if opts.crawl.sitemap_url.is_some() {
            opts.crawl.use_sitemap = true;
        }
        opts
    }
}
