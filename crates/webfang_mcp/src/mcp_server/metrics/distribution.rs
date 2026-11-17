use serde::Serialize;

/// Capacity of the process-lifetime duration reservoir (OBS-M4).
///
/// 1024 samples is a deliberate trade-off, not a default that fell out of a
/// type. One `u64` per sample is 8 KiB for the whole accumulator — negligible
/// beside the 500-domain map it sits next to — and it is large enough that a
/// busy process keeps minutes of history rather than seconds. A 24-hour run
/// will roll the window: past this many events the percentiles describe the
/// RECENT tail, which is the half an SLO cares about, and
/// [`DurationPercentiles::truncated`] plus the `observed` count say so
/// out loud rather than letting a reader assume process-lifetime coverage.
pub const DURATION_SAMPLE_CAPACITY: usize = 1024;

/// Bounded, deterministic ring of duration samples in whole milliseconds
/// (OBS-M4).
///
/// # Why a ring of the last N, not a histogram
///
/// The alternative — fixed log-ish buckets — bounds memory just as well, but
/// it can only ever answer a percentile to within a bucket width, and the
/// bucket edges become part of the published contract. This accumulator exists
/// so an SLO claim ("p95 scrape latency is under 2 s") is a statement about
/// the durations that actually happened. A ring gives EXACT values over its
/// window, and the values it retains are byte-for-byte the integers the
/// `scrape recorded` tracing event already published, so the snapshot and the
/// trace file can never disagree.
///
/// # Determinism
///
/// No RNG (so it is not a statistical reservoir sample), no clock, no
/// `HashMap`: the same sequence of [`record`](Self::record) calls always
/// yields the same [`percentiles`](Self::percentiles). The eviction order is
/// arrival order, so the retained window is exactly the last `capacity`
/// samples. [`record`](Self::record) is `O(1)`, keeping the caller's critical
/// section short (REQ-07).
///
/// # Percentile definition
///
/// **Nearest-rank, 1-indexed, on the retained samples sorted ascending:**
/// `rank = ceil(p * n / 100)` clamped to `[1, n]`, and the answer is the
/// `rank`-th smallest sample. No interpolation, no averaging of neighbours.
///
/// # Reconstructing this from the trace file
///
/// The reservoir holds the same integer milliseconds the `scrape recorded`
/// event publishes as `duration_ms`, so the same numbers come back out of
/// `--trace-file`:
///
/// ```text
/// jq -s '[ .[] | select(.message? == "scrape recorded")
///            | .fields.duration_ms ] | sort
///      | . as $s | ($s|length) as $n
///      | def nr($p): if $n == 0 then null
///                    else $s[((((($p*$n)+99)/100)|floor) - 1)] end;
///        { samples: $n, p50_ms: nr(50), p95_ms: nr(95), p99_ms: nr(99) }'
///    out.jsonl
/// ```
///
/// The one difference is scope, and it is deliberate: this ring answers over
/// its last `capacity` events, while the `jq` pass answers over the whole file
/// — the trace file is the lossless record and this snapshot is the bounded
/// in-process index of it. Both are stated, neither is silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationReservoir {
    /// Ring storage, at most `capacity` entries once filled.
    samples: Vec<u64>,
    /// Next slot to overwrite (only meaningful once `samples` is full).
    next: usize,
    /// Every sample ever recorded, including evicted ones.
    observed: usize,
    /// Documented cap for this reservoir.
    capacity: usize,
}

impl DurationReservoir {
    /// Build an empty reservoir holding at most `capacity` samples.
    ///
    /// A zero capacity is clamped to one: "retain nothing" is not a mode a
    /// percentile reader can be given, it is a misconfiguration, and the
    /// alternative (an empty ring that silently reports `None` forever) hides
    /// it.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::new(),
            next: 0,
            observed: 0,
            capacity: capacity.max(1),
        }
    }

    /// Record one duration in whole milliseconds (OBS-M4).
    ///
    /// `O(1)`: below the cap it appends, at the cap it overwrites the oldest
    /// slot and advances. `next` is kept modded here rather than reset on wrap
    /// so a full ring never re-checks its length.
    pub fn record(&mut self, duration_ms: u64) {
        if self.samples.len() < self.capacity {
            self.samples.push(duration_ms);
        } else {
            self.samples[self.next] = duration_ms;
            self.next = (self.next + 1) % self.capacity;
        }
        self.observed += 1;
    }

    /// Number of samples currently retained (never above [`Self::capacity`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no sample has been retained yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The documented cap for this reservoir.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Every sample ever recorded, including the evicted ones.
    #[must_use]
    pub fn observed(&self) -> usize {
        self.observed
    }

    /// The retained samples, sorted ascending (the percentile input).
    ///
    /// Allocates: this is the snapshot path, called at most once per
    /// `get_scrape_metrics` call, not on the [`record`](Self::record) path.
    #[must_use]
    pub fn sorted_samples(&self) -> Vec<u64> {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        sorted
    }

    /// The published p50 / p95 / p99 plus the metadata needed to read them
    /// honestly (OBS-M4).
    #[must_use]
    pub fn percentiles(&self) -> DurationPercentiles {
        let sorted = self.sorted_samples();
        let n = sorted.len();
        DurationPercentiles {
            p50_ms: Self::nearest_rank(&sorted, 50, n),
            p95_ms: Self::nearest_rank(&sorted, 95, n),
            p99_ms: Self::nearest_rank(&sorted, 99, n),
            samples: n,
            capacity: self.capacity,
            observed: self.observed,
            truncated: self.observed > n,
        }
    }

    /// Nearest-rank percentile: `rank = ceil(p * n / 100)`, 1-indexed, clamped
    /// to `[1, n]`, over `sorted` ascending. `None` when there is no sample —
    /// an empty reservoir reports no percentile rather than a fabricated `0`.
    fn nearest_rank(sorted: &[u64], p: usize, n: usize) -> Option<u64> {
        if n == 0 {
            return None;
        }
        // `div_ceil` on integers, not floats: the same arithmetic the jq
        // recipe in the type rustdoc performs, so the two cannot drift.
        let rank = (p * n).div_ceil(100).clamp(1, n);
        Some(sorted[rank - 1])
    }
}

/// Published percentiles for one [`DurationReservoir`] (OBS-M4).
///
/// `samples` / `capacity` / `observed` / `truncated` travel WITH the numbers
/// on purpose: a percentile without its sample count is how a reader ends up
/// trusting p99 computed from four events. `samples` is the denominator that
/// tells them, and `truncated` says whether the window is the whole lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DurationPercentiles {
    /// Median, or `None` when no sample was retained.
    pub p50_ms: Option<u64>,
    /// 95th percentile, or `None` when no sample was retained.
    pub p95_ms: Option<u64>,
    /// 99th percentile, or `None` when no sample was retained.
    pub p99_ms: Option<u64>,
    /// Samples currently retained in the reservoir (the percentile input).
    pub samples: usize,
    /// The reservoir's documented cap.
    pub capacity: usize,
    /// Samples ever recorded, including the evicted ones.
    pub observed: usize,
    /// `true` when older samples were evicted, so the percentiles describe
    /// the recent window rather than the whole process lifetime.
    pub truncated: bool,
}

impl DurationPercentiles {
    /// The no-sample projection: every rank `None`, nothing retained, nothing
    /// truncated. Used for a cross-product cell that somehow has counters but
    /// no reservoir, so a reader sees "no measurement" instead of `0 ms`.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            p50_ms: None,
            p95_ms: None,
            p99_ms: None,
            samples: 0,
            capacity: 0,
            observed: 0,
            truncated: false,
        }
    }
}
