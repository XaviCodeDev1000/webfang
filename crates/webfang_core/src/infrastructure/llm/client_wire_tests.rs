//! Wire-shape tests for #3092: `response_format_mode` and
//! `max_tokens_parameter`.
//!
//! Split out of `client_tests.rs` to keep both files under the 500-line
//! file-size rule. The mocks and fixtures are shared from the sibling `tests`
//! module, and the module path `client::wire_tests` is declared next to it.

use super::tests::{client_for, mount_capture, test_request};
use super::*;
use crate::domain::providers::{MaxTokensParameter, ResponseFormatMode};
use wiremock::MockServer;

/// The completion body exactly as #3092 found it.
///
/// Kept in the test on purpose: the default body has to be compared
/// byte-for-byte against the pre-existing shape, so a relocated field fails
/// here instead of surfacing as a silent wire drift.
#[derive(serde::Serialize)]
struct LegacyCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    response_format: serde_json::Value,
    temperature: f32,
    max_tokens: usize,
}

fn client_for_with_wire(server: &MockServer, wire: CompletionWireOptions) -> OpenAiLlmClient {
    client_for(server).with_wire_options(wire)
}

#[tokio::test]
async fn default_completion_body_is_byte_identical_to_the_preexisting_wire_shape() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;
    let request = test_request();

    client_for(&server)
        .send_completion(request.clone())
        .await
        .expect("200 + valid JSON succeeds");

    let legacy = LegacyCompletionRequest {
        model: &request.model,
        messages: &request.messages,
        response_format: serde_json::json!({ "type": "json_object" }),
        temperature: 0.0,
        max_tokens: request.max_tokens,
    };
    let expected = serde_json::to_vec(&legacy).expect("the pre-existing shape serializes");
    assert_eq!(
        captured.raw(),
        expected,
        "the default body must not move a single byte, field order included (#3092)"
    );
}

#[tokio::test]
async fn max_completion_tokens_renames_the_budget_field_and_never_sends_both() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    client_for_with_wire(
        &server,
        CompletionWireOptions {
            max_tokens_parameter: MaxTokensParameter::MaxCompletionTokens,
            ..CompletionWireOptions::default()
        },
    )
    .send_completion(test_request())
    .await
    .expect("completion succeeds");

    let body = captured.body();
    assert_eq!(body["max_completion_tokens"], serde_json::json!(64));
    assert!(
        body.get("max_tokens").is_none(),
        "the two budget keys must never travel together: {body}"
    );
    assert_eq!(
        body["response_format"],
        serde_json::json!({ "type": "json_object" }),
        "the token key is independent of the response format"
    );
}

#[tokio::test]
async fn response_format_none_omits_only_that_field() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    client_for_with_wire(
        &server,
        CompletionWireOptions {
            response_format_mode: ResponseFormatMode::None,
            ..CompletionWireOptions::default()
        },
    )
    .send_completion(test_request())
    .await
    .expect("completion succeeds");

    let body = captured.body();
    assert!(
        body.get("response_format").is_none(),
        "mode none must drop the field and nothing else: {body}"
    );
    assert_eq!(body["model"], serde_json::json!("stub-model"));
    assert_eq!(body["temperature"], serde_json::json!(0.0));
    assert_eq!(body["max_tokens"], serde_json::json!(64));
}
