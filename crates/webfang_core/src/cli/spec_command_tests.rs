use super::*;
use crate::domain::options_spec::DefaultValue;

/// Every active spec option must assemble without panicking (legacy heading shape).
#[test]
fn all_groups_assemble_omitted() {
    assert_eq!(
        export_args(Headings::Omitted).len(),
        options_spec::export::GROUP.len()
    );
    assert_eq!(crawler_args(Headings::Omitted).len(), CRAWLER_LAYOUT.len());
    assert_eq!(
        obsidian_args(Headings::Omitted).len(),
        options_spec::obsidian::GROUP.len()
    );
    assert_eq!(ai_args(Headings::Omitted).len(), expected_ai_arg_count());
}

/// Every active spec option must assemble without panicking (runtime heading shape).
#[test]
fn all_groups_assemble_applied() {
    assert_eq!(
        export_args(Headings::Applied).len(),
        options_spec::export::GROUP.len()
    );
    assert_eq!(crawler_args(Headings::Applied).len(), CRAWLER_LAYOUT.len());
    assert_eq!(
        obsidian_args(Headings::Applied).len(),
        options_spec::obsidian::GROUP.len()
    );
    assert_eq!(ai_args(Headings::Applied).len(), expected_ai_arg_count());
}

/// The ids [`ai_args`] renders, in order, for one heading shape.
///
/// Returns owned `String`s: `Arg::get_id` borrows from the `clap::Arg`
/// being built, so the ids cannot outlive the temporary `Vec` this
/// collects from.
fn ai_arg_ids(headings: Headings) -> Vec<String> {
    ai_args(headings)
        .iter()
        .map(|a| a.get_id().as_str().to_string())
        .collect()
}

/// The AI group's EXACT id list, shared by every assertion below so the
/// two cargo configurations cannot drift apart by restating it.
const EXPECTED_AI_IDS: [&str; 3] = ["threshold", "max_chars", "offline"];

/// The AI surface is IDENTICAL in every cargo configuration and for both
/// heading shapes (ADR-0004 slice C, terminal state after paso 6: the
/// flag is pure and there is no cargo feature to split the surface).
///
/// This used to be split: without `ai` the group collapsed to the ungated
/// pair, and each `cfg` arm asserted its own
/// count. That asymmetry was the bug the slice closed — a build without
/// `ai` advertised a different CLI than one with it. One helper, one list,
/// no `cfg`: if a future entry is added, gated or not, both builds show it
/// together and fail here if they do not.
fn assert_ai_layout_in_every_shape(label: &str) {
    for (headings, shape) in [
        (Headings::Omitted, "Omitted"),
        (Headings::Applied, "Applied"),
    ] {
        let owned = ai_arg_ids(headings);
        let ids: Vec<&str> = owned.iter().map(String::as_str).collect();
        assert_eq!(
            ids, EXPECTED_AI_IDS,
            "AI layout {label} must be exactly {EXPECTED_AI_IDS:?} ({shape}); got {ids:?}"
        );
    }
}

/// The AI group renders exactly three entries in every configuration.
#[test]
fn ai_args_has_three_entries() {
    assert_ai_layout_in_every_shape("always");
    assert_eq!(
        expected_ai_arg_count(),
        EXPECTED_AI_IDS.len(),
        "one hand-built `threshold` + two spec-built slots, ungated in \
         every configuration (ADR-0004 slice C)"
    );
}

/// The ungated slots render in EVERY configuration (ADR-0004). Before
/// paso 6 the group collapsed to exactly `max_chars` on a build without
/// the `ai` feature — never to zero, or the chunk-size guard would be
/// invisible and an operator's stale flag would come back as
/// "unexpected argument".
#[test]
fn ungated_ai_args_render_without_the_feature() {
    for headings in [Headings::Omitted, Headings::Applied] {
        let owned = ai_arg_ids(headings);
        let ids: Vec<&str> = owned.iter().map(String::as_str).collect();
        // The chunk-size guard must render in EVERY configuration. If it
        // ever stops, the guard becomes invisible and a stale
        // `--max-chars` comes back as "unexpected argument" instead of a
        // budget the cleaner enforces.
        assert!(
            ids.contains(&"max_chars"),
            "the chunk-size guard must render in every configuration \
             ({headings:?}); got {ids:?}"
        );
        // And since slice C, so must the rest of the group — no AI flag is
        // feature-gated any more.
        assert_eq!(
            ids, EXPECTED_AI_IDS,
            "the AI group must render in full in every configuration \
             ({headings:?}); got {ids:?}"
        );
    }
}

/// A `Uint` default outside any previously-known literal set must
/// assemble without panicking — the clap `default_value` path derives
/// the string from `DefaultValue::Display`, not a hard-coded arm table.
///
/// The synthetic spec reuses the `delay_ms` id so the (separate,
/// intentional) `numeric_binding` parser lookup succeeds; that isolates
/// the default-value rendering path that used to carry the hard-coded
/// `Uint` arm table with an `unreachable!()` wildcard. `999_999` is not
/// in that old table, so the previous implementation panicked here.
#[test]
fn unknown_uint_default_does_not_panic() {
    const SYNTH: OptionSpec = OptionSpec {
        id: "delay_ms",
        long: "synth-new-uint",
        short: None,
        aliases: &[],
        visible_aliases: &[],
        value_name: "SYNTH_NEW_UINT",
        env: None,
        default: Some(DefaultValue::Uint(999_999)),
        nullable: false,
        description_override: None,
        help: "synthetic option for the unknown-Uint default test",
        heading: None,
        kind: ValueKind::uint_unbounded(),
        feature_gate: None,
        value_delimiter: None,
    };
    let arg = build_arg(&SYNTH, Headings::Omitted);
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    assert_eq!(defaults, vec!["999999"]);
}

/// Heading parity BY CONSTRUCTION: the same specs built through
/// [`Headings::Applied`] expose exactly the spec heading on every arg.
#[test]
fn heading_parity_holds_by_construction() {
    for spec in options_spec::export::GROUP {
        let arg = export_args(Headings::Applied)
            .into_iter()
            .find(|a| a.get_id() == spec.id)
            .unwrap_or_else(|| panic!("export arg `{}` missing", spec.id));
        assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
    }
    for slot in CRAWLER_LAYOUT {
        if let CrawlerSlot::Spec(spec) = slot {
            let arg = crawler_args(Headings::Applied)
                .into_iter()
                .find(|a| a.get_id() == spec.id)
                .unwrap_or_else(|| panic!("crawler arg `{}` missing", spec.id));
            assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
        }
    }
    for spec in options_spec::obsidian::GROUP {
        let arg = obsidian_args(Headings::Applied)
            .into_iter()
            .find(|a| a.get_id() == spec.id)
            .unwrap_or_else(|| panic!("obsidian arg `{}` missing", spec.id));
        assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
    }
}

/// The runtime shape carries headings (#932, post-ADR-002): every active
/// spec entry built through the runtime path exposes the same heading as
/// the parity-suite path. This is the post-flip dual of the previous
/// "inert heading" guard; the parity suite already pins the per-arg
/// mapping, this test pins the runtime wiring.
#[test]
fn runtime_shape_applies_spec_headings() {
    let mut specs: Vec<&'static OptionSpec> = options_spec::export::GROUP.iter().collect();
    for slot in CRAWLER_LAYOUT.iter() {
        if let CrawlerSlot::Spec(s) = slot {
            specs.push(*s);
        }
    }
    specs.extend(options_spec::obsidian::GROUP.iter());
    // Every AI spec slot, ungated (ADR-0004 slice C) — asserted in EVERY
    // build, so no configuration can lose an AI flag's heading. `threshold`
    // is the one hand-built arg and carries its heading literally in
    // `manual_threshold`, so it is pinned by `manual_args_carry_headings`
    // rather than through a spec reference.
    for slot in AI_SPEC_LAYOUT.iter() {
        if let AiSlot::Spec(s) = slot {
            specs.push(*s);
        }
    }

    let args: Vec<clap::Arg> = export_args(Headings::Applied)
        .into_iter()
        .chain(crawler_args(Headings::Applied))
        .chain(obsidian_args(Headings::Applied))
        .chain(ai_args(Headings::Applied))
        .collect();

    for spec in specs {
        let arg = args
            .iter()
            .find(|a| a.get_id() == spec.id)
            .unwrap_or_else(|| panic!("runtime arg `{}` missing", spec.id));
        assert_eq!(
            arg.get_help_heading(),
            spec.heading,
            "runtime arg `{}` heading mismatch",
            spec.id
        );
    }
}

/// Manual (structurally-deferred) crawler args carry their layout-adjacent
/// headings unconditionally so no flag renders under the orphan `Options:` bucket (#976).
#[test]
fn manual_args_carry_headings() {
    let expectations = [
        ("concurrency", Some("Discovery")),
        ("rate_limit_burst", Some("Discovery")),
        ("include_patterns", Some("Crawler Settings")),
        ("exclude_patterns", Some("Crawler Settings")),
        ("headers", Some("HTTP Client Settings")),
        ("cookies", Some("HTTP Client Settings")),
    ];
    for (id, expected) in expectations {
        for headings in [Headings::Omitted, Headings::Applied] {
            let arg = crawler_args(headings)
                .into_iter()
                .find(|a| a.get_id() == id)
                .unwrap_or_else(|| panic!("manual arg `{id}` missing for {headings:?}"));
            assert_eq!(
                arg.get_help_heading(),
                expected,
                "manual `{id}` {headings:?}"
            );
        }
    }
}
