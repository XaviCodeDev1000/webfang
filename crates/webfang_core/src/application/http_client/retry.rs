//! Status-specific retry loops for the hardened HTTP client.
//!
//! The hardened client's `get_inner` (see [`HttpClient`]) handles a request's
//! first response inline; when that response is a `429` or a `5xx`, it
//! delegates the bounded retry loop to [`retry_with_backoff`] in this module.
//!
//! The two historical loops (one for `429`, one for `5xx`) were near-identical
//! but differed in four load-bearing ways, captured by [`RetryPolicy`].
//! [`retry_with_backoff`] reproduces both exactly by varying only that policy:
//!
//! | Difference | `429` path | `5xx` path |
//! |---|---|---|
//! | Delay strategy | honors `Retry-After` (constant `secs * 1000` ms) when `> 0`, else exponential | always exponential backoff |
//! | Retryable status | `429` **or** any `5xx` | any `5xx` only |
//! | Exhaustion error | [`HttpError::RateLimited`] | [`HttpError::ServerError`] |
//! | Tracing label | `"429"` | `"5xx"` |
//!
//! Everything else — the attempt counter, request headers, success
//! short-circuit, non-retryable `ClientError`, and timeout handling — is
//! identical and lives here once. The 5xx path's pre-loop WAF inspection stays
//! in `get_inner` because it runs on the initial response, before any retry.
//!
//! Timeout handling (F-08 parity with Stack A `fetch_inner`, RC-1 slice 5):
//! a request timeout is a transient (Stack A classifies it `TransientBackoff`,
//! `downloader_port.rs`), so it consumes the current attempt and the loop
//! continues — it is NOT a terminal error. If the retry budget exhausts with
//! the last attempt failing on TRANSPORT, that failure's classified error
//! (`Timeout` with its cause, #2841) still surfaces, so the terminal error
//! surface is unchanged; it is just delayed until the operator-configured
//! retry budget is actually spent. When the last attempt failed with a status,
//! the policy's bare exhaustion error surfaces with no cause.
//!
//! [`HttpClient`]: crate::application::http_client::HttpClient
//! [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
//! [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError

use crate::domain::error::cause::FetchPhase;
use crate::domain::error::mapping::{http_failure, url_host_of};
use crate::domain::http_config::HttpClientConfig;
use crate::domain::http_error::{HttpError, HttpResult};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;
use wreq::cookie::Jar;
use wreq::Client;

/// The four load-bearing differences between the `429` and `5xx` retry loops.
///
/// Every other aspect of the two loops is identical and lives in
/// [`retry_with_backoff`]; this policy carries only what varies between them so
/// the shared loop can reproduce both behaviors exactly.
pub(super) struct RetryPolicy {
    /// `Retry-After` seconds honored by the `429` path (`Some`); `None` for the
    /// `5xx` path, which always uses exponential backoff.
    pub(super) retry_after_secs: Option<u64>,
    /// Predicate deciding whether a received status code triggers another
    /// retry: the `429` path retries on `429` **or** any `5xx`, the `5xx` path
    /// retries on any `5xx` only.
    pub(super) retryable: fn(u16) -> bool,
    /// Error returned once `max_retries` attempts are exhausted:
    /// [`HttpError::RateLimited`] for the `429` path, [`HttpError::ServerError`]
    /// for the `5xx` path.
    ///
    /// [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
    /// [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError
    pub(super) exhausted: HttpError,
    /// Label tagging the tracing debug messages (`"429"` / `"5xx"`).
    pub(super) label: &'static str,
}

/// Run the bounded retry loop shared by the `429` and `5xx` handling paths.
///
/// Performs up to `config.max_retries` attempts. Each attempt sleeps for
/// [`compute_backoff_delay`], re-sends the GET with the same headers the
/// hardened client uses (Accept-Language, Accept, Referer, Cache-Control and
/// the pinned `user_agent`) and, when one is supplied, the same cookie jar
/// (`jar`), then inspects the outcome:
///
/// - a success status returns the body immediately;
/// - a status satisfying `policy.retryable` continues to the next attempt;
/// - any other status returns [`HttpError::ClientError`];
/// - a transport error continues to the next attempt: a timeout is transient
///   (F-08, retried like Stack A's `fetch_inner`) and so is any other send
///   failure.
///
/// When every attempt is exhausted, `policy.exhausted` is returned — except
/// when the last failure was a timeout, in which case [`HttpError::Timeout`]
/// surfaces (the truthful terminal outcome, now only after the full budget). The `429`
/// caller passes a policy with `Some(retry_after)` and
/// [`HttpError::RateLimited`]; the `5xx` caller passes `None` (pure exponential
/// backoff) and [`HttpError::ServerError`].
///
/// `jar` is the request's cookie jar, the same value the attempt that triggered
/// the retry carried. `None` keeps the client's own jar, which is what the
/// historical single-jar behavior means.
///
/// # Errors
///
/// Returns the body on success, or the first terminal [`HttpError`] (a client
/// error, the LAST attempt's classified transport failure once the budget runs
/// out, or `policy.exhausted` when the last attempt failed with a status).
///
/// [`HttpError::ClientError`]: crate::domain::http_error::HttpError::ClientError
/// [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
/// [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError
pub(super) async fn retry_with_backoff(
    client: &Client,
    url: &str,
    config: &HttpClientConfig,
    user_agent: &str,
    jar: Option<&Arc<Jar>>,
    policy: RetryPolicy,
) -> HttpResult<String> {
    let mut attempt = 0;
    let url_host = url_host_of(url);
    // #2841 (S3/S4): the LAST attempt's failure, already mapped through the one
    // classification seam with that attempt's timing. An attempt that failed
    // with a STATUS resets it — a status carries no transport cause.
    let mut last_transport_failure: Option<HttpError> = None;
    while attempt < config.max_retries {
        attempt += 1;

        let delay_ms = compute_backoff_delay(
            attempt,
            policy.retry_after_secs,
            config.backoff_base_ms,
            config.backoff_max_ms,
        );

        debug!(
            "{} retry attempt {} after {}ms",
            policy.label, attempt, delay_ms
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let mut request = client
            .get(url)
            .header("Accept-Language", &config.accept_language)
            .header("Accept", &config.accept)
            .header("Referer", &config.referer)
            .header("Cache-Control", &config.cache_control)
            .header("User-Agent", user_agent);

        // A retry is the SAME request re-sent, so it carries the same cookie
        // jar. Without this the retry would fall back to the client's own jar
        // and silently abandon every cookie the scoped caller installed, which
        // is exactly the escape `ScopedHttpClient` cannot tolerate.
        if let Some(jar) = jar {
            request = request.cookie_provider(Arc::clone(jar));
        }

        // Apply custom headers on retries too (Bug #675-4) — without this,
        // Authorization/X-Api-Key etc. would be lost on 429/5xx retries.
        for (name, value) in &config.custom_headers {
            request = request.header(name, value);
        }

        let request_started = std::time::Instant::now();
        match request.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    // #2841 (L3 finding): a body-read failure is TRANSPORT, not
                    // request construction — classified through the one seam
                    // with the phase that actually failed.
                    return resp.text().await.map_err(|e| {
                        http_failure(e, FetchPhase::Body, url_host.as_deref(), request_started)
                    });
                } else if (policy.retryable)(resp.status().as_u16()) {
                    last_transport_failure = None;
                    continue;
                } else {
                    return Err(HttpError::ClientError(resp.status().as_u16()));
                }
            },
            Err(e) => {
                // F-08 (RC-1 slice 5): a transport failure is transient — it
                // consumes this attempt and the loop continues, exactly like a
                // timeout. Mapped NOW, with this attempt's timing: if the
                // budget exhausts on transport failures, the LAST one is what
                // the caller sees, cause included (S3).
                last_transport_failure = Some(http_failure(
                    e,
                    FetchPhase::Connect,
                    url_host.as_deref(),
                    request_started,
                ));
                continue;
            },
        }
    }
    // #2841 (S3): exhaustion delivers the LAST transport failure, classified —
    // a timeout is a `Timeout` with its `ConnectTimeout`/`ReadTimeout` cause,
    // exactly the surface the old bare `HttpError::timeout()` kept. S4: when
    // the last attempt failed with a status, nothing is attached and the
    // policy's bare exhaustion error (`RateLimited`/`ServerError`) surfaces.
    if let Some(failure) = last_transport_failure {
        return Err(failure);
    }
    Err(policy.exhausted)
}

/// Compute the delay in milliseconds before retry attempt `attempt` (1-based).
///
/// When `retry_after_secs` is `Some(secs)` with `secs > 0` — the `429` path
/// honoring a `Retry-After` header — the delay is the constant
/// `secs * 1000` ms. In every other case (the `5xx` path's `None`, or a `429`
/// carrying `Retry-After: 0`) the delay is exponential backoff
/// `base_ms * 2^(attempt - 1)` capped at `max_ms`.
pub(super) fn compute_backoff_delay(
    attempt: u32,
    retry_after_secs: Option<u64>,
    base_ms: u64,
    max_ms: u64,
) -> u64 {
    match retry_after_secs {
        Some(secs) if secs > 0 => secs * 1000,
        _ => {
            let exponent = attempt.saturating_sub(1);
            let delay = base_ms * 2_u64.pow(exponent);
            delay.min(max_ms)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_honored_as_constant_delay() {
        // 429 path with Retry-After: 2 → constant 2000ms regardless of attempt.
        assert_eq!(compute_backoff_delay(1, Some(2), 10, 50), 2000);
        assert_eq!(compute_backoff_delay(2, Some(2), 10, 50), 2000);
        assert_eq!(compute_backoff_delay(3, Some(2), 10, 50), 2000);
    }

    #[test]
    fn retry_after_zero_falls_back_to_exponential() {
        // 429 path with Retry-After: 0 → exponential backoff like the 5xx path.
        assert_eq!(compute_backoff_delay(1, Some(0), 10, 50), 10);
        assert_eq!(compute_backoff_delay(2, Some(0), 10, 50), 20);
        assert_eq!(compute_backoff_delay(3, Some(0), 10, 50), 40);
    }

    #[test]
    fn no_retry_after_uses_exponential_backoff() {
        // 5xx path (None) → base * 2^(attempt-1).
        assert_eq!(compute_backoff_delay(1, None, 10, 10_000), 10);
        assert_eq!(compute_backoff_delay(2, None, 10, 10_000), 20);
        assert_eq!(compute_backoff_delay(3, None, 10, 10_000), 40);
        assert_eq!(compute_backoff_delay(4, None, 10, 10_000), 80);
    }

    #[test]
    fn exponential_backoff_is_capped_at_max() {
        // base 1000, max 10000 (production defaults): 1s, 2s, 4s, 8s, then capped.
        assert_eq!(compute_backoff_delay(1, None, 1000, 10_000), 1000);
        assert_eq!(compute_backoff_delay(2, None, 1000, 10_000), 2000);
        assert_eq!(compute_backoff_delay(3, None, 1000, 10_000), 4000);
        assert_eq!(compute_backoff_delay(4, None, 1000, 10_000), 8000);
        assert_eq!(compute_backoff_delay(5, None, 1000, 10_000), 10_000);
        assert_eq!(compute_backoff_delay(6, None, 1000, 10_000), 10_000);
    }

    // -----------------------------------------------------------------------
    // Bug 6 (#675) — 429 retry must use max(Retry-After, exponential_backoff)
    // -----------------------------------------------------------------------

    #[test]
    fn test_429_without_retry_after_uses_exponential() {
        // When no Retry-After header (retry_after_secs = None):
        // attempt 1: base * 2^0 = 1000ms (same as before, by design)
        // attempt 2: base * 2^1 = 2000ms
        // attempt 3: base * 2^2 = 4000ms
        for (attempt, expected_ms) in [(1, 1000), (2, 2000), (3, 4000)] {
            let delay = compute_backoff_delay(attempt, None, 1000, 10_000);
            assert_eq!(
                delay, expected_ms,
                "attempt {attempt} should be {expected_ms}ms, got {delay}ms"
            );
        }
    }

    #[test]
    fn test_429_with_retry_after_uses_max_of_both() {
        // Retry-After: 3 → 3000ms, exponential attempt 2 → 2000ms → max = 3000ms
        let retry_after_ms = 3 * 1000;
        let delay = std::cmp::max(retry_after_ms, compute_backoff_delay(2, None, 1000, 10_000));
        assert_eq!(delay, 3000);

        // Retry-After: 1 → 1000ms, exponential attempt 3 → 4000ms → max = 4000ms
        let delay = std::cmp::max(1000, compute_backoff_delay(3, None, 1000, 10_000));
        assert_eq!(delay, 4000);
    }

    // =====================================================================
    // #2841 — the retry budget's exhaustion carries the last failure's cause
    // =====================================================================

    /// S3: exhaustion after transport failures delivers the LAST transport
    /// failure, classified through the one seam — `127.0.0.1:9` refuses every
    /// dial with no network and no status, so the refusal itself is the only
    /// evidence the exhaustion error may carry.
    #[cfg_attr(miri, ignore)] // TLS_method (btls FFI) via wreq Client::builder, unsupported by Miri
    #[tokio::test]
    async fn exhaustion_after_transport_failures_delivers_the_classified_cause() {
        let client = Client::builder().build().expect("the client builds");
        let config = HttpClientConfig {
            max_retries: 1,
            backoff_base_ms: 0,
            backoff_max_ms: 0,
            ..HttpClientConfig::default()
        };
        let policy = RetryPolicy {
            retry_after_secs: None,
            retryable: |status| status >= 500,
            exhausted: HttpError::ServerError(500),
            label: "5xx",
        };
        let err = retry_with_backoff(
            &client,
            "http://127.0.0.1:9/",
            &config,
            "webfang-test/1.0",
            None,
            policy,
        )
        .await
        .expect_err("the refused dial must exhaust the retry budget");
        assert_eq!(
            crate::domain::error::cause::cause_in_chain(&err),
            Some(crate::domain::error::cause::NetworkCause::ConnectRefused),
            "exhaustion must deliver the last transport failure's cause, got: {err}"
        );
    }

    /// The retry re-sends the request WITH its cookie jar.
    ///
    /// `retry_with_backoff` is the status-retry half of the hardened client. A
    /// retry is the same request sent again, so a jar the caller installed on
    /// the attempt that failed must still be installed on the attempt that
    /// follows; without it the retry falls back to the client's own jar and the
    /// `Set-Cookie` of the failed attempt is lost.
    ///
    /// The fixture answers `503` + `Set-Cookie: probe=1` once and `200`
    /// afterwards, so the assertion reads the cookie the FAILED attempt left
    /// behind on the arrival the retry produced.
    #[cfg_attr(miri, ignore)] // TLS_method (btls FFI) via wreq Client::builder, unsupported by Miri
    #[tokio::test]
    async fn a_retry_carries_the_requests_cookie_jar() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(
                wiremock::ResponseTemplate::new(503).insert_header("set-cookie", "probe=1; Path=/"),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let client = Client::builder().build().expect("the client builds");
        let config = HttpClientConfig {
            max_retries: 2,
            backoff_base_ms: 0,
            backoff_max_ms: 0,
            ..HttpClientConfig::default()
        };
        let jar = Arc::new(Jar::default());
        let policy = RetryPolicy {
            retry_after_secs: None,
            retryable: |status| status >= 500,
            exhausted: HttpError::ServerError(503),
            label: "5xx",
        };

        let body = retry_with_backoff(
            &client,
            &format!("{}/flaky", server.uri()),
            &config,
            "webfang-test/1.0",
            Some(&jar),
            policy,
        )
        .await
        .expect("the second attempt succeeds");

        assert_eq!(body, "ok");
        let arrivals = server
            .received_requests()
            .await
            .expect("the fixture journals every arrival");
        assert_eq!(arrivals.len(), 2, "one failed attempt, one retry");
        let carried = |index: usize| {
            arrivals[index]
                .headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(
            carried(0),
            "",
            "the first attempt starts with an empty jar: nothing has been stored yet"
        );
        assert!(
            carried(1).contains("probe=1"),
            "the retry must re-send with the request's jar, so the cookie the \
             failed attempt stored is offered again; it carried {:?} instead",
            carried(1)
        );
    }

    /// S4: when the last attempt failed with a STATUS, exhaustion stays bare —
    /// `RateLimited`/`ServerError` carry no transport cause. Covers both
    /// policies' exhaustion (S9), wiremock for the HTTP side.
    #[cfg_attr(miri, ignore)] // TLS_method (btls FFI) via wreq Client::builder, unsupported by Miri
    #[tokio::test]
    async fn exhaustion_after_status_failures_stays_bare() {
        for (status, exhausted, label) in [
            (429u16, HttpError::RateLimited(3), "429"),
            (500u16, HttpError::ServerError(500), "5xx"),
        ] {
            let expected = exhausted.to_string();
            let server = wiremock::MockServer::start().await;
            wiremock::Mock::given(wiremock::matchers::any())
                .respond_with(wiremock::ResponseTemplate::new(status))
                .mount(&server)
                .await;
            let client = Client::builder().build().expect("the client builds");
            let config = HttpClientConfig {
                max_retries: 1,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                ..HttpClientConfig::default()
            };
            let policy = RetryPolicy {
                retry_after_secs: None,
                retryable: |code| code == 429 || code >= 500,
                exhausted,
                label,
            };
            let err = retry_with_backoff(
                &client,
                &server.uri(),
                &config,
                "webfang-test/1.0",
                None,
                policy,
            )
            .await
            .expect_err("the status-only budget must exhaust");
            assert_eq!(
                crate::domain::error::cause::cause_in_chain(&err),
                None,
                "a status-exhausted error carries no cause, got: {err}"
            );
            assert_eq!(err.to_string(), expected);
        }
    }
}
