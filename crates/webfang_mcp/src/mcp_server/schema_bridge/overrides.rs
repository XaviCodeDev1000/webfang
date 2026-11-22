//! Advertised-default overrides for the schema bridge (#940 F1/F2): the
//! `DefaultOverride` vocabulary, the per-tool mapping and the application
//! pass that keep the advertised schema TRUTHFUL about runtime. Pure
//! override policy — the rendering lives in the parent `schema_bridge`
//! module.

use serde_json::{json, Map, Value};

use crate::mcp_server::{
    handlers,
    params::{CONCURRENCY_MAX, CONCURRENCY_MIN},
};

/// Shared advertised-default overrides for the two crawl tools: both handlers
/// apply `unwrap_or(CRAWL_SITE_DEFAULT_*)`, never the CLI/spec defaults
/// (2/10) — one helper so the two arms cannot drift apart.
fn crawl_runtime_default_overrides() -> Vec<(&'static str, DefaultOverride)> {
    vec![
        (
            "max_depth",
            DefaultOverride::Set(json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_DEPTH)),
        ),
        (
            "max_pages",
            DefaultOverride::Set(json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_PAGES)),
        ),
    ]
}

/// One advertised-default override, applied AFTER spec derivation (#940 F1/F2).
///
/// The OptionsSpec records CLI defaults; a few MCP handlers apply different
/// runtime defaults. Schema truth outranks spec-default propagation: these
/// overrides adjust what the bridged schema advertises so it matches what the
/// tool actually does.
///
/// `#[non_exhaustive]` (issue #1614): this vocabulary has already grown once in
/// place — `SetBounds` was added by #1294 — so it is a growing set by
/// construction. The attribute makes the next one an S1 / minor change instead
/// of an S2 / major one, and it is free here because the only exhaustive match
/// is [`apply_default_overrides`], in this crate; `non_exhaustive` binds only
/// crates outside the defining one. See `docs/src/mcp-public-surface-policy.md`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DefaultOverride {
    /// Advertise this JSON value as the property's `"default"`, replacing the
    /// spec-derived default. Used to advertise the RUNTIME-effective default.
    Set(Value),
    /// Advertise NO default: the tool forwards the parameter's absence and the
    /// engine decides at runtime — absence is the honest advertisement.
    Unset,
    /// Advertise explicit numeric bounds, overwriting whatever the schemars derive
    /// emitted (#1294 NS-04: a `usize` derives `"minimum": 0`, and for
    /// `concurrency` 0 is a value the validator must reject — #597).
    ///
    /// Applies to properties that are NOT spec-backed too: the override pass runs
    /// on the merged map, so an MCP-only parameter can still be made truthful.
    SetBounds {
        /// Inclusive lower bound to advertise.
        minimum: u64,
        /// Inclusive upper bound to advertise.
        maximum: u64,
    },
}

/// Per-tool advertised-default overrides: `(property name, override)` pairs.
pub type DefaultOverrides = [(&'static str, DefaultOverride)];

/// Runtime-effective advertised-default overrides for one tool's bridged
/// schema, keyed by registered tool name; unknown tools yield an empty set.
/// THE single source of truth for this mapping — both the schema wrappers and
/// the parity tests derive their expectations from here.
#[must_use]
pub fn default_overrides_for_tool(tool: &str) -> Vec<(&'static str, DefaultOverride)> {
    match tool {
        // `crawl_site`: the handler applies `unwrap_or(3)` / `unwrap_or(100)`
        // (handlers/scraping.rs), which differ from the CLI/spec defaults
        // (2/10). Advertise the values the tool actually applies.
        "crawl_site" => crawl_runtime_default_overrides(),
        // `crawl_with_sitemap`: the handler applies
        // `params.max_depth.unwrap_or(CRAWL_SITE_DEFAULT_MAX_DEPTH)` /
        // `params.max_pages.unwrap_or(CRAWL_SITE_DEFAULT_MAX_PAGES)`
        // (#1429 Phase 3) — the same runtime-effective values as
        // `crawl_site`, never the CLI/spec defaults (2/10). Advertise
        // exactly what the tool applies (same helper as the arm above).
        "crawl_with_sitemap" => crawl_runtime_default_overrides(),
        // `scrape_with_options`: an absent `max_pages` is forwarded as `None`,
        // leaving the decision to the engine — advertise no default at all.
        "scrape_with_options" => vec![("max_pages", DefaultOverride::Unset)],
        // `scrape_batch` (RC-1 slice 4): the CLI/spec `--delay-ms` default
        // is 1000 ms, but the tool's runtime default is UNSHROTTLED (an
        // absent `delay_ms` builds no bucket). Schema truth outranks spec
        // propagation: advertise 0 — what the tool actually does.
        "scrape_batch" => vec![
            ("delay_ms", DefaultOverride::Set(json!(0))),
            // NS-04 (#1294): `concurrency` is MCP-only, so no OptionsSpec row ever
            // checked it. Advertise the value the handler actually applies, and the
            // bounds `ScrapeBatchParams::validate` enforces, from the same
            // constants the runtime uses.
            (
                "concurrency",
                DefaultOverride::Set(json!(handlers::scraping::SCRAPE_BATCH_DEFAULT_CONCURRENCY)),
            ),
            (
                "concurrency",
                DefaultOverride::SetBounds {
                    minimum: CONCURRENCY_MIN as u64,
                    maximum: CONCURRENCY_MAX as u64,
                },
            ),
        ],
        _ => Vec::new(),
    }
}

/// Apply advertised-default overrides to a rendered-properties map.
///
/// Public so tests derive expected fragments through the same code path as
/// production. An override naming an unknown property is ignored (with a
/// warning) rather than fabricating a property.
pub fn apply_default_overrides(props: &mut Map<String, Value>, overrides: &DefaultOverrides) {
    for (name, override_) in overrides {
        let Some(Value::Object(prop)) = props.get_mut(*name) else {
            tracing::warn!(
                property = *name,
                "default override targets unknown property"
            );
            continue;
        };
        match override_ {
            DefaultOverride::Set(value) => {
                prop.insert("default".into(), value.clone());
            },
            DefaultOverride::Unset => {
                prop.remove("default");
            },
            DefaultOverride::SetBounds { minimum, maximum } => {
                prop.insert("minimum".into(), json!(minimum));
                prop.insert("maximum".into(), json!(maximum));
            },
        }
    }
}
