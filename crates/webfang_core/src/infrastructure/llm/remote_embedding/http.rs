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
        let endpoint = format!(
            "{}/embeddings",
            self.base_url.as_str().trim_end_matches('/')
        );
        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            // Invariante 1: `auth: None` no manda cabecera. La rama `Some`
            // es la única que puede firmarla, así que no hay forma de que una
            // credencial llegue a una petición declarada anónima — ni siquiera
            // si el entorno tiene una key válida.
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
                    // Un 401 se explica según lo que realmente se mandó. Sin
                    // credencial no es "credencial inválida": mandarle a
                    // rotar una key que nunca existió es el falso positivo
                    // que esta fila evita.
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
                            "el endpoint de embeddings '{}' agotó el tiempo de espera",
                            self.provider_id
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_delay_ms(attempt, None)))
                        .await;
                },
                Err(e) => {
                    return Err(remote_transport(format!(
                        "error de transporte contra el endpoint de embeddings '{}': {}",
                        self.provider_id,
                        render_error_chain(&e)
                    )));
                },
            }
        }
    }

    /// A 401 reportado según si la petición llevaba credencial.
    ///
    /// Sigue siendo [`SemanticError::Inference`] (mismo tipo que cualquier
    /// otro fallo remoto — no se introduce una variante nueva: `error.rs` está
    /// fuera de las superficies de esta unidad). Lo que cambia es el texto en
    /// español y el campo estructurado `auth_source` del log, que es lo que
    /// permite a un operador —o a un matcher programático— distinguir los dos
    /// casos sin parsear el estado HTTP.
    ///
    /// El secreto nunca aparece: `auth_source` es la etiqueta de la fuente
    /// declarada, no la credencial.
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
                    "el endpoint de embeddings '{}' devolvió {status} a una petición sin credencial \
                     (auth source '{}'): el servidor rechazó la petición anónima",
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
                "el endpoint de embeddings '{}' devolvió {status}: rechazó la credencial configurada \
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
                        "la respuesta del endpoint de embeddings '{}' excede 16 MiB",
                        self.provider_id
                    ));
                }
                SemanticError::Inference(format!(
                    "error al leer la respuesta del endpoint de embeddings '{}': {e}",
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
        format!("el endpoint de embeddings '{provider_id}' devolvió el estado {status}"),
        status,
        url,
        retry_after_secs,
    )
}
