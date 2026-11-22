use serde_json::{json, Value};
use webfang_core::domain::options_spec::{crawler, export};

use super::*;

/// Raw schemars rendering of a params struct exactly as rmcp advertises
/// it BEFORE the bridge (root `$schema` retained).
fn raw_derived<P: JsonSchema>() -> Value {
    let mut object = match serde_json::to_value(schemars::schema_for!(P)) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    // rmcp strips the struct doc-comment from the advertised schema.
    object.remove("title");
    object.remove("description");
    Value::Object(object)
}

fn as_value(schema: &Map<String, Value>) -> Value {
    Value::Object(schema.clone())
}

/// Shared runtime-default pins for the two crawl tools: the advertised
/// `default` equals the handler's `unwrap_or` constants (never the
/// CLI/spec 2/10 literals), and the override path provably fired.
/// Both tools share the helper so the contract cannot drift apart;
/// each test below still asserts on its own schema value.
fn assert_crawl_runtime_defaults(schema: &Map<String, Value>) {
    assert_eq!(
        schema["properties"]["max_depth"]["default"],
        json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_DEPTH)
    );
    assert_eq!(
        schema["properties"]["max_pages"]["default"],
        json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_PAGES)
    );
    // The override path actually fired: the fragments differ from the
    // raw spec renderings (which carry the CLI defaults "2"/"10").
    assert_ne!(
        schema["properties"]["max_depth"],
        crawler::MAX_DEPTH.json_schema()
    );
    assert_ne!(
        schema["properties"]["max_pages"],
        crawler::MAX_PAGES.json_schema()
    );
    // Advertised values ARE the constants — not literals that could drift.
    assert_eq!(schema["properties"]["max_depth"]["default"], json!(3));
    assert_eq!(schema["properties"]["max_pages"]["default"], json!(100));
}

/// Proof: `export_file`'s advertised `format` property is rendered from
/// `export::EXPORT_FORMAT`, not from the schemars derive.
///
/// Since SD-01/BC-01 (#1612) the row carries a per-tool
/// `description_override`, so "byte-consistently" now means every
/// dimension the SSOT owns — `type`, the closed `enum`, and `default` —
/// while `description` is the bridge's, by design (#948 F6).
#[test]
fn export_file_advertises_spec_enum_byte_consistently() {
    let schema = export_file_input_schema();
    let rendered = &schema["properties"]["format"];
    let expected = export::EXPORT_FORMAT.json_schema();

    for dimension in ["type", "enum", "default"] {
        assert_eq!(
            rendered[dimension], expected[dimension],
            "format.{dimension} must be the SSOT rendering"
        );
    }
    assert_ne!(
        rendered["description"], expected["description"],
        "the per-tool override must replace the spec help, which reads as a CLI \
         flag that does not exist on this tool"
    );
    assert!(
        rendered["description"]
            .as_str()
            .expect("description")
            .contains("content_format"),
        "the override must name the alias it is an alternative spelling of"
    );

    // Override-path proof: the pre-bridge derive had a different shape
    // (no closed enum / spec default), so the equality above cannot be an
    // accident of the derive.
    let derived = raw_derived::<ExportFileParams>();
    assert_ne!(
        derived["properties"]["format"],
        schema["properties"]["format"]
    );
}

/// Proof (#940 F1): `crawl_site` advertises the RUNTIME-effective
/// defaults — exactly the handler's `unwrap_or` constants — not the
/// CLI/spec defaults (2/10), so advertised schema and runtime cannot
/// drift apart.
#[test]
fn crawl_site_advertises_runtime_effective_defaults() {
    assert_crawl_runtime_defaults(&crawl_site_input_schema());
}

/// Proof (#1429 Phase 3, TRIANGULATE 3.4): `crawl_with_sitemap` advertises
/// the SAME runtime-effective defaults as `crawl_site` — exactly the
/// handler's `unwrap_or` constants (`CRAWL_SITE_DEFAULT_MAX_DEPTH` = 3 /
/// `CRAWL_SITE_DEFAULT_MAX_PAGES` = 100), never the CLI/spec defaults
/// (2/10). Pins truthfulness: runtime defaults == schema-advertised values.
#[test]
fn crawl_with_sitemap_advertises_crawl_site_runtime_defaults() {
    assert_crawl_runtime_defaults(&crawl_with_sitemap_input_schema());
}

/// Proof (#940 F2): `scrape_with_options` advertises NO default for
/// `max_pages` — the handler forwards absence as `None` and the engine
/// decides, so silence is the honest advertisement.
#[test]
fn scrape_with_options_advertises_no_max_pages_default() {
    // Precondition: the spec entry DOES carry a CLI default (Uint(10),
    // rendered as a JSON number by issue #948 F4) that must be stripped
    // for this tool.
    assert_eq!(crawler::MAX_PAGES.json_schema()["default"], json!(10u64));

    let schema = scrape_with_options_input_schema();
    assert_eq!(
        crawler::MAX_PAGES.json_schema()["maximum"],
        json!(100_000u64)
    );
    assert_eq!(
        schema["properties"]["max_pages"]["maximum"],
        json!(100_000u64),
        "bounds still travel from the spec"
    );
    assert!(
        schema["properties"]["max_pages"].get("default").is_none(),
        "engine-decided max_pages must not advertise a default"
    );
}

/// Proof: numeric bounds travel from `NumericPolicy` into the advertised
/// schema (the schemars derive on these fields carries no cap).
#[test]
fn crawl_site_advertises_numeric_policy_caps() {
    let schema = crawl_site_input_schema();
    assert_eq!(
        schema["properties"]["max_pages"]["maximum"],
        json!(100_000u64)
    );
    assert_eq!(schema["properties"]["max_pages"]["minimum"], json!(1u64));
    assert_eq!(schema["properties"]["max_depth"]["maximum"], json!(10u64));

    // The un-overridden derive advertises max_pages WITHOUT any bound:
    let derived = &raw_derived::<CrawlSiteParams>()["properties"]["max_pages"];
    assert!(derived.get("maximum").is_none());
}

/// Proof (#1612, BC-09): every bridged tool's router schema carries its
/// OWN advertised-default overrides.
///
/// Measured red before this invariant existed: `export_file`,
/// `process_export_pipeline`, `scrape_batch` and
/// `get_accessibility_snapshot` all called
/// `merged_input_schema::<P>(PROPS, &[])`, silently dropping the table
/// `default_overrides_for_tool` builds for them. `scrape_batch` therefore
/// published `concurrency` with the derive's `minimum: 0` and no
/// `default` at all, and `delay_ms` with the CLI's 1000 ms while the
/// handler runs unthrottled — the #1294 NS-04 and RC-1 G2 fixes, dead on
/// the wire.
///
/// The existing suites missed it because they re-merged the schema
/// themselves with the overrides supplied by hand, so they asserted the
/// intent rather than the served bytes. The check here is that applying
/// the table to what the ROUTER emits changes nothing: idempotence is the
/// property "the helper passed its overrides", and it needs no
/// per-tool expectation to stay true as the table grows.
#[test]
fn every_bridged_tool_applies_its_own_default_overrides() {
    let router = handlers::build_tool_router();
    for (tool_name, schema_fn) in OVERRIDES {
        let overrides = default_overrides_for_tool(tool_name);
        if overrides.is_empty() {
            continue;
        }
        let route = router
            .map
            .get(*tool_name)
            .unwrap_or_else(|| panic!("{tool_name} must be registered"));
        let served = Value::Object(route.attr.input_schema.as_ref().clone());
        assert_eq!(
            served,
            as_value(&schema_fn()),
            "{tool_name}: the router must serve exactly what its bridge fn builds"
        );
        let mut props = match served.get("properties") {
            Some(Value::Object(map)) => map.clone(),
            other => panic!("{tool_name}: input schema must carry properties, got {other:?}"),
        };
        let before = props.clone();
        apply_default_overrides(&mut props, &overrides);
        assert_eq!(
            before, props,
            "{tool_name}: the served schema does not already carry its own \
             advertised-default overrides"
        );
    }
}

/// Proof: the router wiring actually swaps in the bridge schema — the
/// emitted `inputSchema` equals the bridge output and differs from the
/// raw derive for a spec-covered property.
#[test]
fn build_tool_router_emits_bridge_schemas() {
    let router = handlers::build_tool_router();

    let route = router.map.get("crawl_site").expect("crawl_site registered");
    assert_eq!(
        Value::Object(route.attr.input_schema.as_ref().clone()),
        as_value(&crawl_site_input_schema()),
    );
    let emitted = &route.attr.input_schema["properties"]["max_pages"];
    let derived = &raw_derived::<CrawlSiteParams>()["properties"]["max_pages"];
    assert_ne!(emitted, derived, "router must not emit the raw derive");

    let route = router
        .map
        .get("process_export_pipeline")
        .expect("process_export_pipeline registered");
    assert_eq!(
        Value::Object(route.attr.input_schema.as_ref().clone()),
        as_value(&process_export_pipeline_input_schema()),
    );

    // Non-overridden tools keep their derive untouched.
    let route = router.map.get("scrape_url").expect("scrape_url registered");
    assert_eq!(
        Value::Object(route.attr.input_schema.as_ref().clone()),
        raw_derived::<crate::mcp_server::params::ScrapeUrlParams>(),
    );
}

/// Proof (issue #948 F5): an `Option<T>` MCP param is advertised as
/// `["<inner>", "null"]` (the schemars-derived `Option<T>` shape),
/// not the stricter non-nullable `type: "boolean"` the spec entry's
/// `json_schema()` would otherwise emit. The bridge preserves the
/// derive's `null`-union so the advertised contract matches serde's
/// actual acceptance.
#[test]
fn scrape_with_options_advertises_optional_properties_as_nullable() {
    let schema = scrape_with_options_input_schema();
    let props = &schema["properties"];

    // ignore_robots: Option<bool> in the params struct → ["boolean", "null"].
    let ignore_robots = &props["ignore_robots"];
    assert_eq!(
        ignore_robots["type"],
        json!(["boolean", "null"]),
        "ignore_robots (Option<bool>) must advertise a nullable type, got: {ignore_robots}"
    );

    // selector: Option<String> → ["string", "null"].
    let selector = &props["selector"];
    assert_eq!(
        selector["type"],
        json!(["string", "null"]),
        "selector (Option<String>) must advertise a nullable type, got: {selector}"
    );

    // download_images: Option<bool> → ["boolean", "null"].
    let download_images = &props["download_images"];
    assert_eq!(
        download_images["type"],
        json!(["boolean", "null"]),
        "download_images (Option<bool>) must advertise a nullable type"
    );

    // download_documents: Option<bool> → ["boolean", "null"].
    let download_documents = &props["download_documents"];
    assert_eq!(
        download_documents["type"],
        json!(["boolean", "null"]),
        "download_documents (Option<bool>) must advertise a nullable type"
    );

    // Sanity: the derived shape WAS `["<inner>", "null"]` — the
    // bridge is preserving it, not inventing it.
    let derived = raw_derived::<ScrapeWithOptionsParams>();
    assert_eq!(
        derived["properties"]["ignore_robots"]["type"],
        json!(["boolean", "null"])
    );
    assert_eq!(
        derived["properties"]["selector"]["type"],
        json!(["string", "null"])
    );

    // Non-optional fields (e.g. `url`) keep their non-nullable type.
    assert_eq!(props["url"]["type"], json!("string"));
}

/// Proof (issue #948 F6): a per-tool `description_override` on a
/// `SpecProperty` wins over the spec entry's `help` and over the
/// spec's own `description_override`. The bridge renders the
/// override as the advertised `"description"` so the LLM-facing
/// surface is decoupled from the CLI help wording.
#[test]
fn process_export_pipeline_url_advertises_per_tool_description_override() {
    let schema = process_export_pipeline_input_schema();
    let url = &schema["properties"]["url"];
    let description = url["description"]
        .as_str()
        .expect("description must be a string");
    assert!(
        !description.contains("required unless using a subcommand"),
        "per-tool override must replace the CLI-flavored help text, got: {description}"
    );
    assert!(
        description.contains("Optional URL"),
        "per-tool override must surface the LLM-facing wording, got: {description}"
    );

    // Sanity: the spec entry's `help` is unchanged (the override
    // lives at the bridge layer, not the SSOT).
    assert_eq!(
        crawler::URL.help,
        "URL to scrape (required unless using a subcommand)"
    );
}

/// Proof (issue #948 F6): when neither `SpecProperty::description_override`
/// nor `OptionSpec::description_override` is set, the bridge falls
/// back to the spec's `help` text (no behavior change for entries
/// that haven't been overridden).
#[test]
fn crawl_site_url_falls_back_to_spec_help_when_no_override() {
    let schema = crawl_site_input_schema();
    let url = &schema["properties"]["url"];
    assert_eq!(
        url["description"],
        json!(crawler::URL.help),
        "no override → description must equal spec.help"
    );
}

/// Proof (issue #948 coverage gap): `scrape_batch` now has a bridge
/// table that anchors its `ignore_robots` parameter to the spec
/// entry. Pre-WU5 the field was bridged by nothing — the schemars
/// derive carried the field with no spec-source bounds/description.
/// Post-WU5 the bridge renders it through the spec's `json_schema()`.
#[test]
fn scrape_batch_ignores_robots_renders_through_spec_entry() {
    let schema = scrape_batch_input_schema();
    let rendered = &schema["properties"]["ignore_robots"];

    // The spec entry's `json_schema()` is the SSOT rendering.
    let expected = crawler::IGNORE_ROBOTS.json_schema();
    // F5: the param is `Option<bool>` so the bridge promotes to
    // `["boolean", "null"]`; the expected is the spec's
    // non-nullable form.
    assert_eq!(
        rendered["type"],
        json!(["boolean", "null"]),
        "ignore_robots (Option<bool>) must advertise a nullable type"
    );
    // All other dimensions (description, default) match the spec.
    assert_eq!(rendered["description"], expected["description"]);
    assert_eq!(rendered["default"], expected["default"]);

    // Sanity: the schemars derive alone advertises ignore_robots
    // WITHOUT the spec's boolean default — bridge is the source of
    // truth.
    let derived = raw_derived::<ScrapeBatchParams>();
    let derived_ignore_robots = &derived["properties"]["ignore_robots"];
    assert_ne!(
        rendered["default"], derived_ignore_robots["default"],
        "bridge must override the schemars derive's default with the spec's"
    );
}

/// Proof (AUDIT-02 P6-4): `scrape_batch` advertises `single_page`
/// through the spec entry — CLI `--single-page` parity — with the F5
/// nullability promotion for the `Option<bool>` field.
///
/// Since SD-04 (#1612) the row also carries a per-tool description
/// override, because the handler never reads the field: the spec's
/// generic `--single-page` help reads as a control this tool offers.
/// The type and default still come from the spec; the description is
/// the truth about the handler, which is what the override is for.
#[test]
fn scrape_batch_single_page_renders_through_spec_entry() {
    let schema = scrape_batch_input_schema();
    let rendered = &schema["properties"]["single_page"];

    let expected = crawler::SINGLE_PAGE.json_schema();
    assert_eq!(
        rendered["type"],
        json!(["boolean", "null"]),
        "single_page (Option<bool>) must advertise a nullable type"
    );
    assert_eq!(rendered["default"], expected["default"]);
    assert_ne!(
        rendered["description"], expected["description"],
        "the per-tool override must replace the spec help, or SD-04's no-op \
         field is advertised as a control"
    );
    let description = rendered["description"].as_str().expect("description");
    assert!(
        description.contains("no effect"),
        "the override must say the field is inert, got: {description}"
    );
}

/// Proof (issue #948 coverage gap): `get_accessibility_snapshot` now
/// has a bridge table that anchors its `selector` parameter to the
/// spec entry. Pre-WU5 the field was bridged by nothing.
#[test]
fn get_accessibility_snapshot_selector_renders_through_spec_entry() {
    let schema = get_accessibility_snapshot_input_schema();
    let rendered = &schema["properties"]["selector"];

    let expected = crawler::SELECTOR.json_schema();
    // F5: selector is `Option<String>` so the bridge promotes to
    // `["string", "null"]`.
    assert_eq!(
        rendered["type"],
        json!(["string", "null"]),
        "selector (Option<String>) must advertise a nullable type"
    );
    assert_eq!(rendered["description"], expected["description"]);
    assert_eq!(rendered["default"], expected["default"]);
}

/// Proof: the router wiring actually swaps in the bridge schemas for
/// the two new tools (issue #948 coverage gap). Without the OVERRIDES
/// entries, the router would emit the raw schemars derive.
#[test]
fn build_tool_router_emits_bridge_schemas_for_scrape_batch_and_accessibility() {
    let router = handlers::build_tool_router();

    let route = router
        .map
        .get("scrape_batch")
        .expect("scrape_batch registered");
    let emitted = &route.attr.input_schema["properties"]["ignore_robots"];
    let derived = &raw_derived::<ScrapeBatchParams>()["properties"]["ignore_robots"];
    assert_ne!(
        emitted, derived,
        "scrape_batch router must not emit the raw derive for ignore_robots"
    );

    let route = router
        .map
        .get("get_accessibility_snapshot")
        .expect("get_accessibility_snapshot registered");
    let emitted = &route.attr.input_schema["properties"]["selector"];
    let derived = &raw_derived::<GetAccessibilitySnapshotParams>()["properties"]["selector"];
    assert_ne!(
        emitted, derived,
        "get_accessibility_snapshot router must not emit the raw derive for selector"
    );
}
