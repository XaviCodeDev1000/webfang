//! The degradation path of the jitter RNG (#2749).
//!
//! The OS entropy source cannot be made to fail from a test, so the seed
//! source is a seam ([`SharedRateLimiter::new_with_rng`]) and these tests
//! reach the failure through it.
//!
//! The failure must cost the RANDOMIZATION, not the PACING: the operator
//! asked for a paced run, and a seed failure that removes the pacing removes
//! the protection they asked for.
//!
//! [`SharedRateLimiter::new_with_rng`]: super::SharedRateLimiter::new_with_rng

use std::time::Duration;

use rand::rngs::StdRng;

use super::test_support::{capture_trace, warn_events};
use super::{PacingContext, RateLimiterConfig, SharedRateLimiter};

/// #2749: an unseedable RNG keeps the pacing and warns about the loss.
///
/// The limiter is still built, the token bucket still holds the run back at
/// the configured delay, and the lost randomization is WARNed exactly once.
#[test]
fn an_unseedable_rng_keeps_the_pacing_and_warns() {
    let config = RateLimiterConfig::new(120, 1).with_jitter(500);

    let records = capture_trace(|| async {
        let limiter = SharedRateLimiter::new_with_rng(&config, || {
            Err::<StdRng, &str>("the OS entropy source is unavailable")
        })
        .expect("the seed failure must not abort the limiter");

        // Burn the single burst permit, so the next wait is a REAL one.
        limiter.until_ready().await;
        let ctx = PacingContext::bare("crawl_discovery").with_url("https://example.com/page");
        let waited = limiter.until_ready_observed(&ctx).await;
        assert!(
            waited >= Duration::from_millis(100),
            "the pacing floor must survive the lost randomization, got {waited:?}"
        );
    });

    let warnings = warn_events(&records);
    assert_eq!(
        warnings.len(),
        1,
        "the lost randomization must be warned exactly once"
    );
    let fields = warnings[0]["fields"].as_object().expect("fields");
    assert_eq!(
        fields["jitter_ms"].as_u64(),
        Some(500),
        "the warning names the randomization that was lost"
    );
    assert_eq!(
        fields["delay_ms"].as_u64(),
        Some(120),
        "the warning names the pacing that was kept"
    );
}

/// #2749: `jitter_ms == 0` never consults the seed source.
///
/// Today's behavior, byte for byte: no entropy read, no warning, and the
/// token bucket paces the run alone.
#[test]
fn jitter_zero_never_consults_the_seed_source() {
    let config = RateLimiterConfig::new(120, 1);

    let records = capture_trace(|| async {
        let limiter = SharedRateLimiter::new_with_rng(&config, || -> Result<StdRng, &str> {
            panic!("the seed source must not be consulted when jitter_ms == 0");
        })
        .expect("with jitter 0 the seed source is never consulted");
        limiter.until_ready().await;
    });

    assert!(
        warn_events(&records).is_empty(),
        "jitter 0 must not warn about a degradation that cannot happen"
    );
}
