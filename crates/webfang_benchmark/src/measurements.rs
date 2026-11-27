//! Pure, deterministic measurement helpers for S15 steps 3-4.
//!
//! Challenge signals come from the three production WAF forms; open-loop
//! timelines come from an injected target rate and span samples, never from
//! the wall clock. No crate or dependency is added.

use webfang_core::domain::downloader_port::DownloadError;
use webfang_core::domain::waf::{WafTier, WafVerdict};
use webfang_core::error::ScraperError;

use crate::error::{BenchmarkError, Result};

/// One attempt's WAF classification, reduced to what the rate counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptSignal {
    /// No challenge evidence for this attempt.
    Clean,
    /// A `WafTier::Challenge` event from verdict, download or scraper error.
    WafChallenge,
}

impl From<&WafVerdict> for AttemptSignal {
    fn from(verdict: &WafVerdict) -> Self {
        let challenged = verdict.is_blocked
            && verdict
                .evidences
                .iter()
                .any(|evidence| evidence.tier == WafTier::Challenge);
        if challenged {
            Self::WafChallenge
        } else {
            Self::Clean
        }
    }
}

impl From<&DownloadError> for AttemptSignal {
    fn from(error: &DownloadError) -> Self {
        match error {
            DownloadError::WafChallenge(_) => Self::WafChallenge,
            _ => Self::Clean,
        }
    }
}

impl From<&ScraperError> for AttemptSignal {
    fn from(error: &ScraperError) -> Self {
        match error {
            ScraperError::WafBlocked { .. } => Self::WafChallenge,
            _ => Self::Clean,
        }
    }
}

/// Challenges per attempt. A ratio, never `challenges > 0` (S15 step 3).
#[must_use]
pub fn challenge_rate(signals: &[AttemptSignal]) -> f64 {
    if signals.is_empty() {
        return 0.0;
    }
    let challenges = signals
        .iter()
        .filter(|signal| **signal == AttemptSignal::WafChallenge)
        .count();
    challenges as f64 / signals.len() as f64
}

/// Fixed-rate timeline in milliseconds: attempt `i` starts at
/// `origin_ms + i * (1000 / target_per_sec)`. Pure arithmetic (S15 step 4).
///
/// # Errors
///
/// [`BenchmarkError::Schedule`] when the target rate or origin is not a
/// positive finite number.
pub fn schedule(target_per_sec: f64, count: usize, origin_ms: f64) -> Result<Vec<f64>> {
    if !target_per_sec.is_finite() || target_per_sec <= 0.0 || !origin_ms.is_finite() {
        return Err(BenchmarkError::Schedule(format!(
            "invalid timeline: rate {target_per_sec}, origin {origin_ms}"
        )));
    }
    let step_ms = 1000.0 / target_per_sec;
    Ok((0..count)
        .map(|index| origin_ms + index as f64 * step_ms)
        .collect())
}

/// Nearest-rank percentile over ascending values: 1-based
/// `rank = ceil(p * n / 100)`, no interpolation (ADR-B3).
fn nearest_rank(sorted: &[f64], pct: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (pct * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Observed open-loop latencies for one timeline, reported in memory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyPercentiles {
    /// Nearest-rank 50th percentile in milliseconds.
    pub p50_ms: f64,
    /// Nearest-rank 95th percentile in milliseconds.
    pub p95_ms: f64,
    /// Number of scheduled attempts behind the percentiles.
    pub samples: usize,
}

/// Percentiles of `observed - scheduled` timestamps.
///
/// # Errors
///
/// [`BenchmarkError::Schedule`] when the timelines differ in length, are empty,
/// or contain non-finite values.
pub fn open_loop_percentiles(scheduled: &[f64], observed: &[f64]) -> Result<LatencyPercentiles> {
    if scheduled.is_empty() || scheduled.len() != observed.len() {
        return Err(BenchmarkError::Schedule(format!(
            "timeline length mismatch: {} scheduled, {} observed",
            scheduled.len(),
            observed.len()
        )));
    }
    let mut latencies: Vec<f64> = observed
        .iter()
        .zip(scheduled)
        .map(|(observed_ms, scheduled_ms)| observed_ms - scheduled_ms)
        .collect();
    if latencies.iter().any(|value| !value.is_finite()) {
        return Err(BenchmarkError::Schedule(
            "timeline contains a non-finite timestamp".to_string(),
        ));
    }
    latencies.sort_by(f64::total_cmp);
    Ok(LatencyPercentiles {
        p50_ms: nearest_rank(&latencies, 50),
        p95_ms: nearest_rank(&latencies, 95),
        samples: latencies.len(),
    })
}

/// Build an open-loop timeline from measured span durations: each attempt
/// completes at its scheduled start plus its own observed duration.
///
/// # Errors
///
/// [`BenchmarkError::Schedule`] when the target rate is invalid or the
/// timestamps are not finite.
pub fn from_spans(target_per_sec: f64, durations_ms: &[f64]) -> Result<LatencyPercentiles> {
    let scheduled = schedule(target_per_sec, durations_ms.len(), 0.0)?;
    let observed: Vec<f64> = scheduled
        .iter()
        .zip(durations_ms)
        .map(|(start, duration)| start + duration)
        .collect();
    open_loop_percentiles(&scheduled, &observed)
}
