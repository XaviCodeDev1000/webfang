//! Schema bridge — MCP tool input schemas derived from the OptionsSpec SSOT
//! (ADR-002 slice 4, #940).
//!
//! rmcp generates each tool's advertised `inputSchema` from the schemars
//! derive on its parameter struct, which duplicates bounds and descriptions
//! that already live (authoritatively) in
//! [`webfang_core::domain::options_spec`]. This module overrides the emitted
//! schema for tools whose parameters overlap a spec group: spec-backed
//! properties are rendered through
//! [`webfang_core::domain::options_spec::OptionSpec::json_schema`] while MCP-only
//! properties keep their schemars-derived shape untouched. On top of the spec
//! rendering, per-tool *advertised-default overrides* ([`DefaultOverride`])
//! keep the schema TRUTHFUL about runtime: where a handler applies a different
//! default than the CLI/spec one, the bridge advertises the runtime-effective
//! value (or no default at all when the engine decides) (#940 F1/F2).
//!
//! Layering: this is a one-way seam. The bridge imports
//! `webfang_core::domain::options_spec` only; the core never learns about
//! MCP types.

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use schemars::JsonSchema;
use serde_json::{Map, Value};

use crate::mcp_server::{
    handlers,
    params::{
        CrawlSiteParams, CrawlWithSitemapParams, ExportFileParams, GetAccessibilitySnapshotParams,
        ProcessExportPipelineParams, ScrapeBatchParams, ScrapeWithOptionsParams,
    },
};

mod overrides;
mod properties;

pub use overrides::{
    apply_default_overrides, default_overrides_for_tool, DefaultOverride, DefaultOverrides,
};
pub use properties::{
    SpecProperty, CRAWL_SITE_PROPERTIES, CRAWL_WITH_SITEMAP_PROPERTIES, EXPORT_FILE_PROPERTIES,
    GET_ACCESSIBILITY_SNAPSHOT_PROPERTIES, PROCESS_EXPORT_PIPELINE_PROPERTIES,
    SCRAPE_BATCH_PROPERTIES, SCRAPE_WITH_OPTIONS_PROPERTIES,
};

/// Build the merged input schema for one tool.
///
/// Starts from the schemars-derived schema of the parameter struct `P`
/// (preserving `required`, `additionalProperties`, and every MCP-only
/// property exactly as rmcp would emit it today), then replaces each
/// spec-covered property with its
/// [`webfang_core::domain::options_spec::OptionSpec::json_schema`] rendering,
/// finally applying `default_overrides` on top.
///
/// Nullability preservation (issue #948 F5): when the derived schema
/// advertises a property as `["<inner>", "null"]` (the schemars form for
/// `Option<T>`) and the spec entry is non-nullable (`nullable: false`),
/// the bridge promotes the spec-rendered `type` to the same
/// `["<inner>", "null"]` shape so the advertised contract matches the
/// serde acceptance. `SpecProperty::spec.nullable = true` upgrades the
/// shape unconditionally; the default behavior preserves the derived
/// `null`-union exactly as the derive emitted it.
#[must_use]
pub fn merged_input_schema<P: JsonSchema>(
    properties: &[SpecProperty],
    default_overrides: &DefaultOverrides,
) -> Arc<Map<String, Value>> {
    let mut merged = derived_root::<P>();
    let mut props = match merged.remove("properties") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for property in properties {
        let derived_nullable = derived_type_is_nullable(props.get(property.name));
        let mut rendered = property.spec.json_schema();
        // Promote to ["<inner>", "null"] when the derived shape is
        // nullable AND the spec entry is either explicitly nullable or
        // doesn't override nullability. Without this, a `Option<T>` MCP
        // field is advertised as `"type": "boolean"` while serde still
        // accepts `null` — a stricter contract than the runtime
        // acceptance (#948 F5).
        if derived_nullable && !property.spec.nullable {
            promote_spec_to_nullable(&mut rendered);
        }
        // When the spec is explicitly nullable, the derive path becomes
        // irrelevant — the spec is the source of truth.
        if property.spec.nullable {
            promote_spec_to_nullable(&mut rendered);
        }
        // F6: per-tool description override wins over the spec's
        // own override (and over `help`). Lets the bridge tailor
        // descriptions to the MCP consumer without coupling CLI
        // help wording to the LLM-facing surface.
        if let Some(override_) = property
            .description_override
            .or(property.spec.description_override)
        {
            if let Value::Object(map) = &mut rendered {
                map.insert("description".into(), Value::String(override_.to_owned()));
            }
        }
        props.insert(property.name.to_owned(), rendered);
    }
    apply_default_overrides(&mut props, default_overrides);
    merged.insert("properties".into(), Value::Object(props));
    Arc::new(merged)
}

/// True when the derived property advertises `null` in its `type` field
/// — either as `["<inner>", "null"]` (the schemars `Option<T>` form) or
/// the legacy `{"type": "null"}` (none of our params use it, kept for
/// forward-compat). The schemars 0.8 derive renders the `null` member as
/// the STRING `"null"` (not the JSON `null` literal), so we match both
/// representations.
fn derived_type_is_nullable(value: Option<&Value>) -> bool {
    let Some(Value::Object(prop)) = value else {
        return false;
    };
    match prop.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .any(|t| matches!(t, Value::Null) || t == &Value::String("null".to_owned())),
        Some(Value::String(_)) => false,
        _ => false,
    }
}

/// Promote a rendered schema's `type` to the `["<inner>", "null"]` form
/// (no-op when the inner type is missing or already a union). Emits the
/// `null` member as the STRING `"null"` to match the schemars 0.8
/// convention (JSON Schema 2020-12 accepts both forms; the LLM
/// consumers we target already understand the string form).
///
/// Public so the CLI↔MCP parity test (`options_spec_parity_test`) can
/// apply the same nullability promotion to its expected fragment that
/// the bridge applies to the production rendering (#948 F5).
pub fn promote_spec_to_nullable(rendered: &mut Value) {
    let Value::Object(map) = rendered else {
        return;
    };
    match map.get("type") {
        Some(Value::Array(_)) => {}, // already nullable
        Some(Value::String(inner)) => {
            let inner = inner.clone();
            map.insert(
                "type".into(),
                Value::Array(vec![Value::String(inner), Value::String("null".to_owned())]),
            );
        },
        _ => {},
    }
}

/// Schemars-derived root object for `P`, mirroring rmcp 1.8's
/// `validate_and_strip` normalization exactly: top-level `title` and
/// `description` removed; everything else (including root `$schema`) kept.
fn derived_root<P: JsonSchema>() -> Map<String, Value> {
    let Ok(value) = serde_json::to_value(schemars::schema_for!(P)) else {
        // Unreachable for schemars-generated schemas (plain JSON types
        // only); degrade to an empty root rather than panic the server.
        tracing::error!("schemars schema failed to serialize; emitting empty input schema");
        return Map::new();
    };
    let mut object = match value {
        Value::Object(map) => map,
        _ => return Map::new(),
    };
    object.remove("title");
    object.remove("description");
    object
}

fn crawl_site_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("crawl_site");
    merged_input_schema::<CrawlSiteParams>(CRAWL_SITE_PROPERTIES, &overrides)
}

fn crawl_with_sitemap_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("crawl_with_sitemap");
    merged_input_schema::<CrawlWithSitemapParams>(CRAWL_WITH_SITEMAP_PROPERTIES, &overrides)
}

fn scrape_with_options_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("scrape_with_options");
    merged_input_schema::<ScrapeWithOptionsParams>(SCRAPE_WITH_OPTIONS_PROPERTIES, &overrides)
}

fn export_file_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("export_file");
    merged_input_schema::<ExportFileParams>(EXPORT_FILE_PROPERTIES, &overrides)
}

fn process_export_pipeline_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("process_export_pipeline");
    merged_input_schema::<ProcessExportPipelineParams>(
        PROCESS_EXPORT_PIPELINE_PROPERTIES,
        &overrides,
    )
}

fn scrape_batch_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("scrape_batch");
    merged_input_schema::<ScrapeBatchParams>(SCRAPE_BATCH_PROPERTIES, &overrides)
}

fn get_accessibility_snapshot_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("get_accessibility_snapshot");
    merged_input_schema::<GetAccessibilitySnapshotParams>(
        GET_ACCESSIBILITY_SNAPSHOT_PROPERTIES,
        &overrides,
    )
}

type InputSchemaFn = fn() -> Arc<Map<String, Value>>;

/// Tools whose advertised input schema is overridden by the bridge.
///
/// Every entry names a registered tool and the bridge function producing its
/// merged schema. A missing route (tool renamed without updating this table)
/// logs a warning instead of failing the router build.
const OVERRIDES: &[(&str, InputSchemaFn)] = &[
    ("crawl_site", crawl_site_input_schema as InputSchemaFn),
    (
        "crawl_with_sitemap",
        crawl_with_sitemap_input_schema as InputSchemaFn,
    ),
    (
        "scrape_with_options",
        scrape_with_options_input_schema as InputSchemaFn,
    ),
    ("export_file", export_file_input_schema as InputSchemaFn),
    (
        "process_export_pipeline",
        process_export_pipeline_input_schema as InputSchemaFn,
    ),
    ("scrape_batch", scrape_batch_input_schema as InputSchemaFn),
    (
        "get_accessibility_snapshot",
        get_accessibility_snapshot_input_schema as InputSchemaFn,
    ),
];

/// Replace the advertised `inputSchema` of every overridden tool in `router`.
pub fn apply_overrides(router: &mut ToolRouter<handlers::McpHandler>) {
    for (tool_name, schema_fn) in OVERRIDES {
        match router.map.get_mut(*tool_name) {
            Some(route) => route.attr.input_schema = schema_fn(),
            None => tracing::warn!(
                tool = *tool_name,
                "schema_bridge override target not found in tool router"
            ),
        }
    }
}

#[cfg(test)]
#[path = "schema_bridge_tests.rs"]
mod tests;
