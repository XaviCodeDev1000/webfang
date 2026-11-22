//! Declarative spec-property tables for the schema bridge (ADR-002 slice 4,
//! #940): the per-tool `SpecProperty` rows that anchor each tool's
//! spec-backed parameters to their [`OptionSpec`] entries. Pure data — the
//! rendering lives in the parent `schema_bridge` module.

use webfang_core::domain::options_spec::{crawler, export, OptionSpec};

/// One spec-backed property of an overridden tool input schema.
#[derive(Debug, Clone, Copy)]
pub struct SpecProperty {
    /// MCP wire name of the property. May differ from [`OptionSpec::id`]
    /// when the tool's parameter name predates the spec identifier.
    pub name: &'static str,
    /// OptionsSpec entry describing the property's type, bounds, default,
    /// and description.
    pub spec: &'static OptionSpec,
    /// Per-tool description override for the advertised MCP schema
    /// (issue #948 F6). When `Some`, the bridge uses this string as the
    /// property's `"description"` instead of `spec.help`. Most rows leave
    /// this `None` and fall back to the spec's `description_override`
    /// (or `help` if neither is set).
    pub description_override: Option<&'static str>,
}

const fn prop(name: &'static str, spec: &'static OptionSpec) -> SpecProperty {
    SpecProperty {
        name,
        spec,
        description_override: None,
    }
}

/// `crawl_site`: every parameter overlaps the crawler spec group.
pub const CRAWL_SITE_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    prop("max_depth", &crawler::MAX_DEPTH),
    prop("max_pages", &crawler::MAX_PAGES),
    // P6-2 run parity (#1343): the tool forwards the strategy to the engine;
    // spec default (static) matches the handler's `unwrap_or_default`, so no
    // advertised-default override is needed. `session_pool`/`checkpoint_dir`
    // stay MCP-only (schemars shape, like `concurrency`).
    prop("js_strategy", &crawler::JS_STRATEGY),
];

/// `crawl_with_sitemap`: `url`, `max_depth`, `max_pages` overlap the crawler
/// spec group (#1429 Phase 3, D4). `sitemap_url` stays MCP-only (schemars
/// shape, like `concurrency`). No `js_strategy` row: the handler does not
/// forward it yet (defaults until the params grow it — same comment block as
/// the `crawl_site` options copy in `handlers/scraping.rs`).
pub const CRAWL_WITH_SITEMAP_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    prop("max_depth", &crawler::MAX_DEPTH),
    prop("max_pages", &crawler::MAX_PAGES),
];

/// `scrape_with_options`: every parameter overlaps the crawler spec group.
///
/// `max_pages` carries a per-tool description override (SD-03, #1612). The
/// handler copies it into `ScraperConfig` and
/// `scraper_service::scrape_with_config` never reads it — the one-page use
/// case this tool serves has nothing to count. Bounds stay enforced
/// (1..=100_000 from `crawler::MAX_PAGES`), so the advertisement says the
/// field is inert rather than pretending it is a control.
///
/// **Deferred to #1614 (compat policy), deliberately not done here:**
/// removing `max_pages` from this tool's advertised schema, or rejecting a
/// value above 1, is a breaking change for MCP consumers and is that issue's
/// call. This table states the decision so the next reader inherits it
/// instead of re-deriving it.
pub const SCRAPE_WITH_OPTIONS_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    SpecProperty {
        name: "max_pages",
        spec: &crawler::MAX_PAGES,
        description_override: Some(
            "Accepted and bounds-checked (1-100000), but it has no effect here: this tool \
             fetches exactly the `url` you give it and never discovers or follows links, so \
             every call returns one page. To crawl more than one page use crawl_site or \
             crawl_with_sitemap.",
        ),
    },
    prop("selector", &crawler::SELECTOR),
    prop("download_images", &crawler::DOWNLOAD_IMAGES),
    prop("download_documents", &crawler::DOWNLOAD_DOCUMENTS),
    prop("ignore_robots", &crawler::IGNORE_ROBOTS),
];

/// `export_file`: only `format` overlaps (`export::EXPORT_FORMAT`; wire name
/// differs from the spec id `export_format`). Path/blob params are MCP-only.
///
/// **SD-01 / BC-01 (#1612), decided as far as it can be here.** Three names
/// are in play and they are not three fields:
///
/// | name | where it comes from | advertised |
/// |---|---|---|
/// | `content_format` | the params struct, and `required` in the derive | yes |
/// | `format` | this bridge row, mapped from spec id `export_format` | yes |
/// | `export_format` | the OptionsSpec id, which nothing accepts on this tool | no |
///
/// Collapsing the two advertised spellings into one is a *removal* from the
/// advertised schema and belongs to #1614, so neither is removed here. What
/// this row does is stop the bridge from advertising `format` as if it were an
/// extra field: the override says it is the alternative spelling of the
/// required `content_format`, and that sending both is a duplicate-field
/// error. Measured, not assumed -- `serde_json::from_value` on
/// `{"content_format": …, "format": …}` fails with
/// ``duplicate field `content_format` ``.
pub const EXPORT_FILE_PROPERTIES: &[SpecProperty] = &[SpecProperty {
    name: "format",
    spec: &export::EXPORT_FORMAT,
    description_override: Some(
        "Export format (jsonl, vector, auto). Alternative spelling of the required \
         `content_format` property -- send exactly one of the two, never both: they are \
         aliases of a single field, and sending both is a duplicate-field error. The spec \
         id is `export_format`, which this tool does not accept.",
    ),
}];

/// `process_export_pipeline`: `url` and `format` overlap their spec entries.
/// The `url` row carries a per-tool description override (issue #948 F6) —
/// the spec's `help` reads "URL to scrape (required unless using a
/// subcommand)" which is CLI-flavored and confusing for LLM consumers of
/// this optional-parameter tool.
pub const PROCESS_EXPORT_PIPELINE_PROPERTIES: &[SpecProperty] = &[
    SpecProperty {
        name: "url",
        spec: &crawler::URL,
        description_override: Some(
            "Optional URL to scrape before exporting. Omit to skip scraping and run the export stage on previously-saved content.",
        ),
    },
    SpecProperty {
        name: "format",
        spec: &export::EXPORT_FORMAT,
        // SD-02 (#1612): the params accept `pipeline_format`, `format` AND
        // `export_format`; the schema advertised two of the three. The third
        // is NOT added here: a third advertised spelling next to the other
        // two is a wider target for a client that sends two of them and eats
        // a duplicate-field error. Declaring the accepted set in prose fixes
        // the discoverability gap without widening the surface, and whether
        // the set stays at three is #1614's compat decision.
        description_override: Some(
            "Export format (jsonl, vector, auto). Also accepted as `pipeline_format` and \
             `export_format`; send exactly one of the three, never two -- they are aliases \
             of a single field and sending two is a duplicate-field error.",
        ),
    },
];

/// `scrape_batch`: the `ignore_robots` field overlaps `crawler::GROUP`
/// (issue #948 coverage gap — the tool was registered in WU3 without
/// a bridge table). Other params (`urls`, `concurrency`) are MCP-only
/// and stay on the schemars derive. `single_page` overlaps the spec too
/// (AUDIT-02 P6-4 CLI `--single-page` parity) and carries a per-tool
/// description override (SD-04, #1612): the handler never reads it, so
/// the advertisement says so. `delay_ms` overlaps the spec too (RC-1
/// slice 4, G2 pacing parity — the SAME token-bucket cadence the CLI
/// `--delay-ms` drives).
///
/// **Deferred to #1614 (compat policy), deliberately not done here:** removing
/// `single_page` from the advertised schema, or turning `single_page: false`
/// into a rejection, are both breaking changes for MCP consumers. The
/// alternative — adding a crawl-expansion mode to `scrape_batch` so the flag
/// acquires a meaning — is a feature, not a schema fix, and belongs to
/// whichever issue takes it.
pub const SCRAPE_BATCH_PROPERTIES: &[SpecProperty] = &[
    prop("ignore_robots", &crawler::IGNORE_ROBOTS),
    SpecProperty {
        name: "single_page",
        spec: &crawler::SINGLE_PAGE,
        description_override: Some(
            "Accepted for CLI --single-page parity, but it has no effect here: \
             scrape_batch already scrapes exactly one page per input URL and never expands \
             into linked pages, so there is no crawl mode for it to disable.",
        ),
    },
    prop("delay_ms", &crawler::DELAY_MS),
];

/// `get_accessibility_snapshot`: the `selector` field overlaps
/// `crawler::GROUP` (issue #948 coverage gap). `url`, `interactive_only`,
/// and `format` stay on the schemars derive.
pub const GET_ACCESSIBILITY_SNAPSHOT_PROPERTIES: &[SpecProperty] =
    &[prop("selector", &crawler::SELECTOR)];
