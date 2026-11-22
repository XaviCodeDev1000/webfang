use super::super::fixtures::load_fixture;
use super::super::parse_embedding_body;
use crate::error::SemanticError;

#[test]
fn parse_happy_fixture_maps_vectors_one_to_one_in_order() {
    let parsed = parse_embedding_body(&load_fixture("embeddings_happy.json"), 2)
        .expect("happy fixture parses");
    let vecs = parsed.vectors;
    assert_eq!(vecs.len(), 2, "one vector per input, in order");
    assert_eq!(vecs[0], vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
    assert_eq!(vecs[1], vec![0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1]);
}

#[test]
fn parse_tolerant_fixture_ignores_unknown_fields() {
    let parsed = parse_embedding_body(&load_fixture("embeddings_tolerant.json"), 1)
        .expect("vendor extras must be ignored");
    let vecs = parsed.vectors;
    assert_eq!(vecs.len(), 1);
    assert_eq!(vecs[0].len(), 8, "non-384d dims pass through");
    assert!((vecs[0][0] - 0.5).abs() < f32::EPSILON);
}

#[test]
fn parse_empty_data_is_semantic_error() {
    let err = parse_embedding_body(&load_fixture("embeddings_empty.json"), 1)
        .expect_err("empty data must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "empty data must be Inference, got: {err}"
    );
}

#[test]
fn parse_count_mismatch_is_semantic_error() {
    let err = parse_embedding_body(&load_fixture("embeddings_happy.json"), 1)
        .expect_err("2 vectors for 1 input must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "count mismatch must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains('2'),
        "mismatch must name the served count, got: {err}"
    );
}

#[test]
fn parse_empty_vector_is_semantic_error() {
    let err = parse_embedding_body(r#"{"data": [{"index": 0, "embedding": []}]}"#, 1)
        .expect_err("empty vector must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "empty vector must be Inference, got: {err}"
    );
}

#[test]
fn parse_invalid_json_is_semantic_error() {
    let err = parse_embedding_body("not json", 1).expect_err("invalid body must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "invalid body must be Inference, got: {err}"
    );
}
