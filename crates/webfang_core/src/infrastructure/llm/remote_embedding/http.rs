use std::time::Duration;

use chrono::Utc;

use super::proxy_auth;
use super::render_error_chain;
use super::retry::{backoff_delay_ms, retry_after_delay};
use super::wire::EmbeddingRequest;
use super::RemoteEmbeddingAdapter;
use super::{parse_embedding_body, EMBEDDING_MAX_ATTEMPTS, EMBEDDING_MAX_BODY_BYTES};
use crate::error::ai_failure::{remote_status, remote_transport};
use crate::error::SemanticError;

impl RemoteEmbeddingAdapter {
    /// POST one batch with the bounded retry classification: timeouts, 429
    /// (honoring `max(Retry-After, backoff)`) and 5xx retry with backoff;
    /// other 4xx and builder errors are terminal as-is; exhaustion reports
    /// the last observed status, never a hardcoded one.
    pub(super) async fn post_embeddings(
        &self,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        self.post_embeddings_with(texts, &self.extra_body).await
    }

    /// Same guard chain with an explicit body map. Vault search passes the
    /// `query_extra_body` overlay here; indexing and the startup probe use
    /// `post_embeddings` with the base map.
    pub(super) async fn post_embeddings_with(
        &self,
        texts: &[String],
        extra_body: &crate::domain::providers::ExtraBody,
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let endpoint = format!(
            "{}/embeddings",
            self.base_url.as_str().trim_end_matches('/')
        );
        let request = EmbeddingRequest {
            extra_body,
            model: &self.model,
            input: texts,
            encoding_format: "float",
        };
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            // Invariant 1: `auth: None` sends no header. The `Some` branch
            // is the only one that can sign it, so a credential can never reach
            // a request declared anonymous — not even when the environment
            // holds a valid key.
            let mut pending = self.http.post(&endpoint);
            if let Some(key) = &self.api_key {
                pending = pending.bearer_auth(key.expose_secret());
            }
            match pending.json(&request).send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if response.status().is_success() {
                        return self.read_and_parse(response, texts.len()).await;
                    }
                    if status == 429 || (500..=599).contains(&status) {
                        let raw_retry_after = response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok());
                        let retry_after = retry_after_delay(raw_retry_after, Utc::now());
                        if attempt >= EMBEDDING_MAX_ATTEMPTS {
                            let retry_secs = if status == 429 {
                                retry_after.map(|delay| delay.as_secs())
                            } else {
                                None
                            };
                            return Err(last_status_error(
                                &self.provider_id,
                                status,
                                self.base_url.as_str(),
                                retry_secs,
                            ));
                        }
                        // A header we could not honor is a server
                        // instruction we are about to ignore — say so,
                        // with the raw value, instead of silently
                        // downgrading to exponential backoff. An
                        // *oversized* header is reported one level down,
                        // inside `retry_after_delay`, which is the only
                        // place that still knows the requested magnitude.
                        if let Some(raw) = raw_retry_after {
                            if retry_after.is_none() {
                                tracing::warn!(
                                    provider_id = %self.provider_id,
                                    status,
                                    raw = %raw,
                                    reason = "neither delay-seconds nor a parsable HTTP-date",
                                    "remote embedding retry-after discarded; falling back to exponential backoff"
                                );
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(backoff_delay_ms(
                            attempt,
                            retry_after,
                        )))
                        .await;
                        continue;
                    }
                    // A 401 is explained by what was actually sent. Without a
                    // credential it is not an "invalid credential": telling the
                    // operator to rotate a key that never existed is the false
                    // positive this row avoids.
                    if status == 401 {
                        return Err(self.unauthorized_error(status));
                    }
                    // 407 is a terminal proxy-auth failure: zero retries.
                    if status == 407 {
                        return Err(proxy_auth::proxy_auth_error(
                            &self.provider_id,
                            self.proxy_auth_configured,
                            status,
                            self.base_url.as_str(),
                        ));
                    }
                    return Err(last_status_error(
                        &self.provider_id,
                        status,
                        self.base_url.as_str(),
                        None,
                    ));
                },
                Err(e) if e.is_timeout() => {
                    if attempt >= EMBEDDING_MAX_ATTEMPTS {
                        return Err(remote_transport(format!(
                            "embedding endpoint '{}' timed out",
                            self.provider_id
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_delay_ms(attempt, None)))
                        .await;
                },
                Err(e) => {
                    return Err(remote_transport(format!(
                        "transport error against embedding endpoint '{}': {}",
                        self.provider_id,
                        render_error_chain(&e)
                    )));
                },
            }
        }
    }

    /// A 401 reported according to whether the request carried a credential.
    ///
    /// It remains [`SemanticError::Inference`] (the same type as any other
    /// remote failure — no new variant is introduced: `error.rs` is outside
    /// this unit's surfaces). What changes is the English text and the
    /// structured `auth_source` field in the log, which let an operator —or a
    /// programmatic matcher— distinguish the two cases without parsing the
    /// HTTP status.
    ///
    /// The secret never appears: `auth_source` is the label of the declared
    /// source, not the credential.
    fn unauthorized_error(&self, status: u16) -> SemanticError {
        if self.api_key.is_none() {
            tracing::warn!(
                provider_id = %self.provider_id,
                status,
                auth_source = self.auth_source,
                credential_sent = false,
                "remote embedding endpoint rejected an anonymous request"
            );
            return remote_status(
                format!(
                    "embedding endpoint '{}' returned {status} for a request without a credential \
                     (auth source '{}'): the server rejected the anonymous request",
                    self.provider_id, self.auth_source
                ),
                status,
                self.base_url.as_str(),
                None,
            );
        }
        tracing::warn!(
            provider_id = %self.provider_id,
            status,
            auth_source = self.auth_source,
            credential_sent = true,
            "remote embedding endpoint rejected the configured credential"
        );
        remote_status(
            format!(
                "embedding endpoint '{}' returned {status}: rejected the configured credential \
                 (auth source '{}')",
                self.provider_id, self.auth_source
            ),
            status,
            self.base_url.as_str(),
            None,
        )
    }

    /// Bounded body read (16 MiB cap) plus the tolerant wire parse, with the
    /// 1:1 input↔vector count check. Accumulates usage metrics.
    async fn read_and_parse(
        &self,
        response: wreq::Response,
        expected_inputs: usize,
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let body = crate::domain::body_cap::read_body_capped(response, EMBEDDING_MAX_BODY_BYTES)
            .await
            .map_err(|e| {
                if matches!(e, crate::domain::http_error::HttpError::BodyTooLarge { .. }) {
                    return SemanticError::Inference(format!(
                        "embedding endpoint '{}' response exceeds 16 MiB",
                        self.provider_id
                    ));
                }
                SemanticError::Inference(format!(
                    "error reading the embedding endpoint '{}' response: {e}",
                    self.provider_id
                ))
            })?;
        let parsed = parse_embedding_body(&body, expected_inputs)?;
        if let Some(usage) = parsed.usage {
            self.usage_prompt_tokens.fetch_add(
                usage.prompt_tokens as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.usage_total_tokens.fetch_add(
                usage.total_tokens as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.usage_request_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!(
                prompt_tokens = usage.prompt_tokens,
                total_tokens = usage.total_tokens,
                accumulated_prompt = self
                    .usage_prompt_tokens
                    .load(std::sync::atomic::Ordering::Relaxed),
                accumulated_total = self
                    .usage_total_tokens
                    .load(std::sync::atomic::Ordering::Relaxed),
                accumulated_requests = self
                    .usage_request_count
                    .load(std::sync::atomic::Ordering::Relaxed),
                "remote embedding usage accumulated"
            );
        }
        Ok(parsed.vectors)
    }
}

/// Terminal/retries-exhausted status error naming the last observed status.
///
/// The observed status, dialed endpoint and (for 429) honored `Retry-After`
/// travel in the tag so the single `From<SemanticError>` conversion can
/// classify without inventing a status that was never seen.
fn last_status_error(
    provider_id: &str,
    status: u16,
    url: &str,
    retry_after_secs: Option<u64>,
) -> SemanticError {
    remote_status(
        format!("embedding endpoint '{provider_id}' returned status {status}"),
        status,
        url,
        retry_after_secs,
    )
}
