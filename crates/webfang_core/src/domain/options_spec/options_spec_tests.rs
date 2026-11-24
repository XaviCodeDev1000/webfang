use super::{ai, crawler, export, schema_object, BoundError, DefaultValue, OptionSpecError};
use serde_json::json;

#[test]
fn every_export_option_has_a_wellformed_identity() {
    for opt in export::GROUP {
        assert!(
            opt.id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "id `{}` must be snake_case (clap arg id / MCP property)",
            opt.id
        );
        assert!(
            opt.long
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
            "long `{}` must be kebab-case",
            opt.long
        );
    }
    assert_eq!(
        export::GROUP.len(),
        16,
        "export group must cover all ExportArgs fields"
    );
}

#[test]
fn parse_uint_rejects_non_numeric_input_with_exact_message() {
    let err = export::CPU_CORES
        .parse_uint("abc")
        .expect_err("must reject text");
    assert_eq!(err.to_string(), "`abc` is not a valid integer");
}

#[test]
fn parse_uint_rejects_zero_with_exact_message() {
    let err = export::CPU_CORES
        .parse_uint("0")
        .expect_err("zero cores is invalid");
    assert_eq!(err.to_string(), "cpu-cores must be > 0");
}

#[test]
fn parse_uint_accepts_positive_values() {
    assert_eq!(export::CPU_CORES.parse_uint("8").expect("valid"), 8);
    assert_eq!(
        export::BATCH_CONCURRENCY.parse_uint("16").expect("valid"),
        16
    );
}

#[test]
fn memory_size_bound_rejects_zero_with_structured_error() {
    // F7: `check_bound` returns a typed `BoundError`, not the verbose
    // message the CLI path uses. The English wording IS the
    // MCP-stable rendering every MCP validator relies on.
    let err = export::RAM_BUDGET
        .check_bound(0)
        .expect_err("zero budget is invalid");
    assert!(
        matches!(err, BoundError::MinViolated { min: 1 }),
        "expected MinViolated {{ min: 1 }}, got {err:?}"
    );
    assert_eq!(err.to_string(), "must be at least 1");
    assert_eq!(export::RAM_BUDGET.check_bound(2048).expect("valid"), 2048);

    // The CLI path keeps the verbatim message via
    // `parse_uint` (which goes through `OptionSpecError::Bound`).
    // `RAM_BUDGET` is a `MemorySize`, not `Uint`, so the CLI path is
    // `parse_error` instead — we cover the same English wording via
    // `parse_uint` on a `Uint` entry below.
}

#[test]
fn non_integer_kinds_have_no_integer_parser() {
    let err = export::OUTPUT
        .parse_uint("output")
        .expect_err("Path kind has no uint parser");
    assert!(matches!(err, OptionSpecError::UnsupportedKind { .. }));
}

#[test]
fn crawler_group_covers_every_non_deferred_crawler_args_field() {
    assert_eq!(crawler::GROUP.len(), 43);
    for opt in crawler::GROUP {
        assert!(
            opt.id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "id `{}` must be snake_case",
            opt.id
        );
    }
}

#[test]
fn feature_gated_entries_mirror_the_compile_time_cfg() {
    // A gate that DECLARES itself must name a real cargo feature, and
    // `active()` must agree with the actual compilation configuration.
    //
    // `CLEAN_AI` left this loop in ADR-0004 slice C: the semantic cleaner no
    // longer needs ONNX to exist, so `--clean-ai` is ungated and renders in
    // every build. Its capability half has no cargo feature left to name
    // (step 6 removes the `ai` feature): a requested `--clean-ai` whose
    // cleaner wiring failed surfaces at run time as the export flow's
    // ConfigError naming `--embedding-provider` (exit 78).
    let gated: Vec<&'static super::OptionSpec> = crawler::GROUP
        .iter()
        .chain(ai::GROUP.iter())
        .chain(export::GROUP.iter())
        .filter(|s| s.feature_gate.is_some())
        .collect();

    // Derived, not hard-coded: the ungating in ADR-0004 slice C removed
    // entries from this set, so a fixed list would rot silently. Asserting
    // it is NON-EMPTY is what stops the loop from passing vacuously if a
    // future ungating empties it — that is the one failure mode a
    // derived set cannot report on its own.
    assert!(
        !gated.is_empty(),
        "at least one spec must stay feature-gated; if this set is empty \
         the invariant below would pass without checking anything"
    );

    for opt in gated {
        let gate = opt.feature_gate.expect("filtered to gated entries above");
        // A declared gate must name a REAL feature, and `active()` must
        // mirror the actual compilation configuration for it.
        assert_eq!(
            opt.active(),
            match gate {
                // Same FIXED value as `gate_active` (ADR-0004 step 6):
                // the `ai` feature no longer exists, so the gate is
                // inactive in EVERY configuration.
                "ai" => false,
                "adaptive-selectors" => cfg!(feature = "adaptive-selectors"),
                other => panic!(
                    "gated spec `{}` names unknown feature `{other}`; \
                     unknown gates must fail closed, and this test is \
                     where that is pinned",
                    opt.id
                ),
            },
            "active() must mirror cfg for gate `{gate}` (spec `{}`)",
            opt.id
        );
    }

    // The NEW contract, asserted rather than assumed (ADR-0004): the
    // cleaner is OFFERED without the `ai` cargo feature. This is stronger
    // than the entry's absence from the loop above — it pins both halves,
    // that `clean_ai` declares no gate and that it is therefore active in
    // EVERY configuration. Re-gating `CLEAN_AI` fails HERE, naming the
    // promise it would have broken.
    assert_eq!(
        crawler::CLEAN_AI.feature_gate,
        None,
        "CLEAN_AI must stay ungated (ADR-0004 slice C): the semantic cleaner \
         needs no ONNX build, so the flag must render in every configuration"
    );
    assert!(
        crawler::CLEAN_AI.active(),
        "an ungated spec is always active, so --clean-ai must be offered in \
         every build"
    );

    // Ungated entries are always active.
    assert!(export::OUTPUT.active());
}

#[test]
fn unknown_feature_gates_fail_closed() {
    let gated = super::OptionSpec {
        feature_gate: Some("not-a-real-feature"),
        ..export::OUTPUT
    };
    assert!(!gated.active());
}

#[test]
fn verbatim_template_renders_byte_exactly_for_migrated_crawler_flags() {
    let parse_err = crawler::MAX_PAGES
        .parse_uint("abc")
        .expect_err("must reject text");
    assert_eq!(
        parse_err.to_string(),
        "'abc' is not a valid number for --max-pages"
    );

    let bound_err = crawler::MAX_PAGES
        .parse_uint("0")
        .expect_err("zero pages is invalid");
    assert_eq!(
        bound_err.to_string(),
        "--max-pages must be >= 1 (0 leaves no pages to scrape)"
    );

    let timeout_parse = crawler::TIMEOUT_SECS.parse_error("xyz");
    assert_eq!(
        timeout_parse.to_string(),
        "'xyz' is not a valid number for --timeout-secs"
    );
}

#[test]
fn metadata_only_numeric_entries_accept_zero_and_any_value() {
    // `policy: None` records "no bound today"; the spec must not invent
    // one, and the schema must not advertise a fabricated minimum.
    assert_eq!(crawler::DELAY_MS.parse_uint("0").expect("unbounded"), 0);
    let schema = crawler::DELAY_MS.json_schema();
    assert_eq!(schema["type"], "integer");
    assert!(
        schema.get("minimum").is_none(),
        "unbounded entries must not fabricate a minimum"
    );
    // Parse failures keep the canonical integer message.
    let err = crawler::DELAY_MS
        .parse_uint("abc")
        .expect_err("must reject text");
    assert_eq!(err.to_string(), "`abc` is not a valid integer");
}

#[test]
fn crawler_max_depth_keeps_zero_valid_and_enforces_the_shared_cap() {
    // 0 = seed-only semantics stay valid; 10 is the inclusive cap
    // shared with the MCP tool schema (#940).
    assert_eq!(crawler::MAX_DEPTH.parse_uint("0").expect("seed-only"), 0);
    assert_eq!(crawler::MAX_DEPTH.parse_uint("10").expect("cap"), 10);
    let err = crawler::MAX_DEPTH
        .parse_uint("11")
        .expect_err("above the shared cap");
    assert_eq!(err.to_string(), "--max-depth must be <= 10");
    // Parse failures keep the canonical integer message.
    let err = crawler::MAX_DEPTH
        .parse_uint("abc")
        .expect_err("must reject text");
    assert_eq!(err.to_string(), "`abc` is not a valid integer");
}

#[test]
fn crawler_max_pages_enforces_the_shared_upper_cap() {
    assert_eq!(crawler::MAX_PAGES.parse_uint("1").expect("min"), 1);
    assert_eq!(
        crawler::MAX_PAGES
            .parse_uint("100000")
            .expect("inclusive cap"),
        100_000
    );
    let err = crawler::MAX_PAGES
        .parse_uint("100001")
        .expect_err("above the shared cap");
    assert_eq!(err.to_string(), "--max-pages must be <= 100000");
}

#[test]
fn json_schema_describes_enum_options_with_canonical_variants() {
    let schema = export::EXPORT_FORMAT.json_schema();
    assert_eq!(schema["type"], "string");
    assert_eq!(schema["enum"], json!(["jsonl", "vector", "auto"]));
    assert_eq!(schema["default"], "jsonl");
    assert!(schema["description"].as_str().is_some());
}

#[test]
fn json_schema_encodes_numeric_bounds_from_the_spec() {
    let schema = export::CPU_CORES.json_schema();
    assert_eq!(schema["type"], "integer");
    assert_eq!(schema["minimum"], 1);
    assert!(schema.get("maximum").is_none());

    let ram = export::RAM_BUDGET.json_schema();
    assert_eq!(ram["format"], "byte-size");
    assert_eq!(ram["minimumBytes"], 1);

    // Capped entries advertise both bounds (shared CLI↔MCP caps).
    let depth = crawler::MAX_DEPTH.json_schema();
    assert_eq!(depth["minimum"], 0);
    assert_eq!(depth["maximum"], 10);
    let pages = crawler::MAX_PAGES.json_schema();
    assert_eq!(pages["minimum"], 1);
    assert_eq!(pages["maximum"], 100_000);
}

#[test]
fn json_schema_covers_boolean_path_and_text_kinds() {
    // ELASTIC carries a typed `default: Bool(false)` (issue #948 F4
    // strict) — the advertised schema emits a native JSON boolean, not
    // a string, closing the type-inconsistency drift the bridge
    // previously inherited.
    let elastic = export::ELASTIC.json_schema();
    assert_eq!(elastic["type"], "boolean");
    assert_eq!(elastic["default"], false);
    assert_eq!(elastic["default"], json!(false));

    let output = export::OUTPUT.json_schema();
    assert_eq!(output["type"], "string");

    let vectors = export::OUTPUT_VECTORS.json_schema();
    assert_eq!(vectors["type"], "string");
}

/// Issue #948 F4 drift-killer: integer- and boolean-kind properties must
/// advertise their `default` as a native JSON number/bool, NEVER as a
/// JSON string. The pre-F4 renderer emitted `"default": "2"` for an
/// `integer` kind — inconsistent with `"type": "integer"` and
/// type-strict MCP validators.
#[test]
fn json_schema_emits_native_typed_defaults_for_uint_and_bool() {
    // MAX_PAGES (crawler) is `Uint(10)` — schema must carry `2`-as-number.
    let pages = crawler::MAX_PAGES.json_schema();
    assert_eq!(pages["type"], "integer");
    assert_eq!(
        pages["default"],
        json!(10u64),
        "MAX_PAGES default must be a JSON number, not a string"
    );
    assert!(
        pages["default"].is_number(),
        "MAX_PAGES default must be a JSON number, got: {}",
        pages["default"]
    );

    // MAX_DEPTH (crawler) is `Uint(2)` — same invariant.
    let depth = crawler::MAX_DEPTH.json_schema();
    assert_eq!(depth["type"], "integer");
    assert_eq!(depth["default"], json!(2u64));
    assert!(depth["default"].is_number());

    // Bool entry: DOM_PREPRUNE is `Bool(true)`.
    let dom = crawler::DOM_PREPRUNE.json_schema();
    assert_eq!(dom["type"], "boolean");
    assert_eq!(
        dom["default"],
        json!(true),
        "DOM_PREPRUNE default must be a JSON bool, not a string"
    );
    assert!(dom["default"].is_boolean());

    // Sanity: textual entries (no `default` set) omit `default` from
    // the schema; entries with `default: Some(DefaultValue::Str(...))`
    // emit a JSON string so the CLI parity path stays drift-free.
    let text = crawler::SELECTOR.json_schema();
    assert_eq!(text["type"], "string");
    assert_eq!(text["default"], json!("body"));

    // F4 back-compat: `DefaultValue` `Display` mirrors the canonical
    // CLI string form the args parity tests assert.
    assert_eq!(DefaultValue::Uint(10).to_string(), "10");
    assert_eq!(DefaultValue::Bool(false).to_string(), "false");
    assert_eq!(DefaultValue::Str("jsonl").to_string(), "jsonl");
}

/// Issue #948 F7 drift-killer: `check_bound` returns the typed
/// `BoundError` variants so MCP validators can format the
/// MCP-stable wording without re-comparing the value or re-reading
/// the spec's `NumericPolicy`. The pre-F7 code re-implemented
/// `value < policy.min` / `value > policy.max` in every validator
/// (`params.rs::validate_max_pages`, `validate_max_depth`) — the
/// structured error collapses the duplication.
#[test]
fn check_bound_returns_typed_bound_error_variants() {
    // Below the inclusive minimum: MinViolated carries the bound.
    let err = crawler::MAX_PAGES
        .check_bound(0)
        .expect_err("zero pages is invalid");
    assert!(
        matches!(err, BoundError::MinViolated { min: 1 }),
        "expected MinViolated {{ min: 1 }}, got {err:?}"
    );
    assert_eq!(err.to_string(), "must be at least 1");

    // Above the inclusive cap: MaxViolated carries the cap.
    let err = crawler::MAX_PAGES
        .check_bound(100_001)
        .expect_err("above the cap is invalid");
    assert!(
        matches!(err, BoundError::MaxViolated { max: 100_000 }),
        "expected MaxViolated {{ max: 100000 }}, got {err:?}"
    );
    assert_eq!(err.to_string(), "must be at most 100000");

    // MAX_DEPTH keeps zero valid (seed-only) and caps at 10.
    assert_eq!(crawler::MAX_DEPTH.check_bound(0).expect("seed-only"), 0);
    let err = crawler::MAX_DEPTH
        .check_bound(11)
        .expect_err("above the cap is invalid");
    assert!(matches!(err, BoundError::MaxViolated { max: 10 }));
    assert_eq!(err.to_string(), "must be at most 10");

    // Inclusive boundaries accept (no re-implementation needed).
    assert_eq!(crawler::MAX_PAGES.check_bound(1).expect("min"), 1);
    assert_eq!(
        crawler::MAX_PAGES.check_bound(100_000).expect("cap"),
        100_000
    );

    // Unbounded entries accept any value (policy = None).
    assert_eq!(crawler::DELAY_MS.check_bound(0).expect("unbounded"), 0);
    assert_eq!(
        crawler::DELAY_MS.check_bound(u64::MAX).expect("unbounded"),
        u64::MAX
    );
}

#[test]
fn schema_object_aggregates_the_whole_group_by_id() {
    let obj = schema_object(export::GROUP);
    let entries = obj.as_object().expect("aggregate must be an object");
    assert_eq!(entries.len(), 16);
    for opt in export::GROUP {
        assert!(
            entries.contains_key(opt.id),
            "missing `{}` in aggregate",
            opt.id
        );
    }
    assert_eq!(
        obj["pipeline_output"]["enum"],
        json!(["jsonl", "none"]),
        "PipelineOutputFormat variants must match clap's kebab-case rendering"
    );
    assert_eq!(
        obj["format"]["enum"],
        json!(["markdown", "json", "text"]),
        "OutputFormat variants must match clap's kebab-case rendering"
    );
}
