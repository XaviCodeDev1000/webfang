use super::tests_support::*;
use super::*;

/// REQ-08: fixed durations serialize to an exact, byte-stable pretty JSON
/// string (BTreeMap sorted keys, flat `average_duration_ms`).
#[test]
fn snapshot_serialization_exact() {
    let mut m = ScrapeMetrics::default();
    record_fixture(&mut m);
    let snap = m.snapshot();

    let expected = [
        "{",
        "  \"total_events\": 3,",
        "  \"success_count\": 2,",
        "  \"error_count\": 1,",
        "  \"partial_count\": 0,",
        "  \"domains\": {",
        "    \"a.com\": {",
        "      \"events\": 2,",
        "      \"pages\": 8,",
        "      \"errors\": 0",
        "    },",
        "    \"b.com\": {",
        "      \"events\": 1,",
        "      \"pages\": 0,",
        "      \"errors\": 1",
        "    }",
        "  },",
        "  \"tools\": {",
        "    \"scrape_url\": 3",
        "  },",
        "  \"average_duration_ms\": 200.0,",
        "  \"duration_percentiles\": {",
        "    \"p50_ms\": 200,",
        "    \"p95_ms\": 300,",
        "    \"p99_ms\": 300,",
        "    \"samples\": 3,",
        "    \"capacity\": 1024,",
        "    \"observed\": 3,",
        "    \"truncated\": false",
        "  },",
        "  \"tool_domains\": {",
        "    \"scrape_url\": {",
        "      \"a.com\": {",
        "        \"events\": 2,",
        "        \"pages\": 8,",
        "        \"errors\": 0,",
        "        \"duration_percentiles\": {",
        "          \"p50_ms\": 100,",
        "          \"p95_ms\": 200,",
        "          \"p99_ms\": 200,",
        "          \"samples\": 2,",
        "          \"capacity\": 32,",
        "          \"observed\": 2,",
        "          \"truncated\": false",
        "        }",
        "      },",
        "      \"b.com\": {",
        "        \"events\": 1,",
        "        \"pages\": 0,",
        "        \"errors\": 1,",
        "        \"duration_percentiles\": {",
        "          \"p50_ms\": 300,",
        "          \"p95_ms\": 300,",
        "          \"p99_ms\": 300,",
        "          \"samples\": 1,",
        "          \"capacity\": 32,",
        "          \"observed\": 1,",
        "          \"truncated\": false",
        "        }",
        "      }",
        "    }",
        "  }",
        "}",
    ]
    .join("\n");

    assert_eq!(
        serde_json::to_string_pretty(&snap).expect("snapshot must serialize"),
        expected,
        "serialization must be byte-stable"
    );
}

/// Additive-stability guard: the fields that existed before OBS-M4/OBS-M6
/// must keep their exact bytes AND their exact position, so a consumer
/// that indexes positionally is not broken by two new members.
#[test]
fn pre_existing_snapshot_fields_are_byte_identical() {
    let mut m = ScrapeMetrics::default();
    record_fixture(&mut m);
    let json = serde_json::to_string_pretty(&m.snapshot()).expect("must serialize");
    let legacy_prefix = [
        "{",
        "  \"total_events\": 3,",
        "  \"success_count\": 2,",
        "  \"error_count\": 1,",
        "  \"partial_count\": 0,",
        "  \"domains\": {",
        "    \"a.com\": {",
        "      \"events\": 2,",
        "      \"pages\": 8,",
        "      \"errors\": 0",
        "    },",
        "    \"b.com\": {",
        "      \"events\": 1,",
        "      \"pages\": 0,",
        "      \"errors\": 1",
        "    }",
        "  },",
        "  \"tools\": {",
        "    \"scrape_url\": 3",
        "  },",
        "  \"average_duration_ms\": 200.0,",
    ]
    .join("\n");
    assert!(
        json.starts_with(&legacy_prefix),
        "OBS-M4/OBS-M6 must be purely additive; the legacy prefix drifted:\n{json}"
    );
}
