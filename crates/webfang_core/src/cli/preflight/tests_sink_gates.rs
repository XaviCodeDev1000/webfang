use super::*;

// ========================================================================
// #695 — --elastic sink availability preflight
// ========================================================================

/// `--elastic` disabled: no sink is required, check passes.
#[test]
fn elastic_disabled_ok() {
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = false;
    assert!(check_elastic_sink(&opts).is_ok());
}

/// `--elastic` + `--output-vectors`: the JSONL stream sink exists in
/// every build, so the check passes regardless of `persistence`.
#[test]
fn elastic_with_output_vectors_ok() {
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    opts.elastic.output_vectors = Some("vectors.jsonl".into());
    assert!(check_elastic_sink(&opts).is_ok());
}

/// `--elastic` alone without the `persistence` feature: no wirable
/// sink — must fail fast with a config error naming the flag.
#[cfg(not(feature = "persistence"))]
#[test]
fn elastic_without_persistence_errors() {
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    opts.elastic.output_vectors = None;
    let err =
        check_elastic_sink(&opts).expect_err("no sink available without persistence must fail");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--elastic"),
            "config error must name the flag, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// `--elastic` alone with the `persistence` feature: the SQLite sink
/// is wirable, check passes.
#[cfg(feature = "persistence")]
#[test]
fn elastic_with_persistence_ok() {
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    opts.elastic.output_vectors = None;
    assert!(check_elastic_sink(&opts).is_ok());
}

#[test]
fn db_path_without_elastic_is_rejected() {
    // #799: --db-path without --elastic is a silent no-op; reject it up
    // front so the user does not lose data unknowingly.
    let mut opts = CrawlOptions::default();
    opts.elastic.db_path = Some(std::path::PathBuf::from("/tmp/test.db"));
    assert!(check_db_path_requires_elastic(&opts).is_err());
}

#[test]
fn db_path_with_elastic_is_allowed() {
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    opts.elastic.db_path = Some(std::path::PathBuf::from("/tmp/test.db"));
    assert!(check_db_path_requires_elastic(&opts).is_ok());
}
