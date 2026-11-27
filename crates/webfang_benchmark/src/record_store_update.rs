//! T0 (#3081) — the permanent `RecordStore::update` measurement.
//!
//! `update` is the live write path of `--resume`: it takes the store lock,
//! loads the domain file, mutates in memory and writes it back atomically.
//! Today it reads the whole state file **twice** per call — once in
//! `load_under_lock`, once in `save_locked` → `existing_created_at` — which is
//! the quantity the later optimization (T8) changes, so this module measures
//! both the latency and the read volume.
//!
//! Measurement integrity (L29, `std::hint::black_box` docs): `black_box` is a
//! best-effort pessimization, never a correctness device, and it only affects
//! its OUTPUT, so an expression fed to it may still be hoisted. The loop below
//! wraps the result of the call and the inputs the measured code must consume
//! (the store handle, the loaded-record count, the record payload) and then
//! VERIFIES the persisted effect afterwards — the assertion, not `black_box`,
//! is what proves each sample did real work.
//!
//! Reads come from `/proc/self/io` deltas (`rchar`, `read_bytes`) around the
//! measured window: a documented kernel interface that attributes to this
//! process without `strace` or privileges. It is Linux-only, so off Linux the
//! metric is reported as `unavailable` and the run still succeeds.
//!
//! Statistics reuse the house convention (ADR-B3, [`crate::aggregate`]):
//! nearest-rank percentile over ascending-sorted samples, no interpolation.
//! The headline `ms/update` is the MEDIAN (p50): one scheduler hiccup moves a
//! mean and leaves the median alone, and T8's threshold must compare a central
//! value, not a tail. p95 is reported beside it so the tail is visible.

use std::path::Path;
use std::time::Instant;

use webfang_core::domain::exporter::{DomainRecords, RawRecord, RecordStoreError};
use webfang_core::infrastructure::export::RecordStore;

use crate::aggregate::nearest_rank;
use crate::error::{BenchmarkError, Result};

/// The two store sizes the benchmark reports (requirement: 1k and 10k).
pub const SIZES: [usize; 2] = [1_000, 10_000];

/// Samples per size. Odd so the nearest-rank median is a real observation
/// rather than the midpoint of two; 21 keeps the 10k run a few seconds long.
pub const DEFAULT_SAMPLES: u32 = 21;

/// Domain key of the benchmark's throwaway store. Never a real domain: the
/// state file only ever lives in the caller's scratch directory.
pub const DOMAIN_KEY: &str = "bench-record-store-update.test";

/// Fixed timestamp base for the seeded records: 13 decimal digits, so neither
/// the seed nor a sample changes the file's byte length (and therefore not the
/// `reads/update` normalization either).
const BASE_CLOCK_MS: i64 = 1_700_000_000_000;

/// Offset applied to the touched record. Still 13 digits.
const TOUCHED_CLOCK_OFFSET_MS: i64 = 1_000_000;

/// Run id stamped on every seeded record. Constant: the benchmark injects its
/// clock, it never reads one.
const RUN_ID: &str = "bench-t0";

/// `rchar` and `read_bytes` from `/proc/self/io`.
///
/// `rchar` counts every byte returned by `read(2)` (page-cache hits included),
/// which is what makes it usable for "how many times was the file read".
/// `read_bytes` counts bytes that actually hit the block device, so it is
/// usually 0 for a file this process just wrote — reported for the record,
/// never used to claim a read count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoCounters {
    /// Bytes returned by `read(2)`/`pread(2)` so far.
    pub rchar: u64,
    /// Bytes fetched from the block device so far.
    pub read_bytes: u64,
}

/// One size's measurement: latency percentiles plus read volume.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateReport {
    /// Records seeded in the store for this measurement.
    pub records: usize,
    /// Measured `update` calls.
    pub samples: u32,
    /// Size of the seeded state file before the measured window, in bytes.
    pub state_bytes: u64,
    /// Nearest-rank median `update` duration, milliseconds.
    pub ms_p50: f64,
    /// Nearest-rank 95th percentile `update` duration, milliseconds.
    pub ms_p95: f64,
    /// Full-file reads per `update`, derived from `rchar`; `None` off Linux.
    pub reads_per_update: Option<f64>,
    /// `rchar` delta divided by the number of samples; `None` off Linux.
    pub rchar_per_update: Option<u64>,
    /// `read_bytes` delta divided by the number of samples; `None` off Linux.
    pub read_bytes_per_update: Option<u64>,
}

/// Measure `samples` `RecordStore::update` calls over a store holding
/// `records` records, rooted at `dir`.
///
/// The seed write and the final verification happen OUTSIDE the measured
/// window; the window contains exactly the `samples` update calls.
///
/// # Errors
///
/// - [`BenchmarkError::RecordStore`] when `records`/`samples` is zero, when a
///   seed or sample transaction fails, or when the post-run verification does
///   not find every record (a sample that did nothing).
/// - [`BenchmarkError::Io`] when the seeded state file cannot be inspected.
pub fn measure(dir: &Path, records: usize, samples: u32) -> Result<UpdateReport> {
    if records == 0 || samples == 0 {
        return Err(BenchmarkError::RecordStore(
            "records and samples must both be greater than zero".to_string(),
        ));
    }

    let store = RecordStore::new(DOMAIN_KEY).with_state_dir(dir.to_path_buf());
    seed(&store, records)?;
    let state_bytes = std::fs::metadata(store.state_path())?.len();

    // Sample inputs are built before the window: formatting a URL is harness
    // cost, not the cost of `update`.
    let plan = (0..samples)
        .map(|sample| sample_input(records, sample))
        .collect::<Result<Vec<_>>>()?;

    let io_before = read_io_counters();
    let mut elapsed_ms = Vec::with_capacity(samples as usize);
    for (key, payload) in &plan {
        let payload = std::hint::black_box(payload);
        let mut mutate = |current: &mut DomainRecords| {
            // The measured code must consume the records it loaded, not a
            // length it knew before the call.
            std::hint::black_box(current.len());
            current.insert(key.clone(), payload.clone());
            Ok(())
        };
        let store = std::hint::black_box(&store);
        let started = Instant::now();
        let outcome = store.update(&mut mutate);
        elapsed_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        std::hint::black_box(&outcome);
        outcome.map_err(store_error)?;
    }
    let io_after = read_io_counters();

    verify(&store, records, samples)?;

    elapsed_ms.sort_by(f64::total_cmp);
    let (reads, rchar, read_bytes) = read_metrics(io_before, io_after, state_bytes, samples);
    Ok(UpdateReport {
        records,
        samples,
        state_bytes,
        ms_p50: nearest_rank(&elapsed_ms, 50),
        ms_p95: nearest_rank(&elapsed_ms, 95),
        reads_per_update: reads,
        rchar_per_update: rchar,
        read_bytes_per_update: read_bytes,
    })
}

/// The stable, greppable summary line of one size (T8 asserts against it).
///
/// Shape: `record_store_update records=<n> samples=<n> state_bytes=<n>
/// ms_per_update_p50=<ms> ms_per_update_p95=<ms> reads_per_update=<f64|unavailable>
/// rchar_per_update=<bytes|unavailable> read_bytes_per_update=<bytes|unavailable>`.
#[must_use]
pub fn summary_line(report: &UpdateReport) -> String {
    let reads = report
        .reads_per_update
        .map_or_else(|| "unavailable".to_string(), |value| format!("{value:.3}"));
    let rchar = report
        .rchar_per_update
        .map_or_else(|| "unavailable".to_string(), |value| value.to_string());
    let read_bytes = report
        .read_bytes_per_update
        .map_or_else(|| "unavailable".to_string(), |value| value.to_string());
    format!(
        "record_store_update records={} samples={} state_bytes={} \
         ms_per_update_p50={:.3} ms_per_update_p95={:.3} reads_per_update={reads} \
         rchar_per_update={rchar} read_bytes_per_update={read_bytes}",
        report.records, report.samples, report.state_bytes, report.ms_p50, report.ms_p95,
    )
}

/// Parse `/proc/self/io` content into the two counters this benchmark uses.
///
/// Deliberately strict: a missing or unparsable counter is `None` (reported as
/// `unavailable`), never a zero that would look like a free update.
#[must_use]
pub fn parse_proc_io(text: &str) -> Option<IoCounters> {
    let mut rchar = None;
    let mut read_bytes = None;
    for line in text.lines() {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        match name.trim() {
            "rchar" => rchar = Some(value.parse::<u64>().ok()?),
            "read_bytes" => read_bytes = Some(value.parse::<u64>().ok()?),
            _ => {},
        }
    }
    Some(IoCounters {
        rchar: rchar?,
        read_bytes: read_bytes?,
    })
}

/// Current `/proc/self/io` counters; `None` when the interface is absent or
/// unreadable (any non-Linux platform, or a restricted container).
#[cfg(target_os = "linux")]
#[must_use]
pub fn read_io_counters() -> Option<IoCounters> {
    let text = std::fs::read_to_string("/proc/self/io").ok()?;
    parse_proc_io(&text)
}

/// Current `/proc/self/io` counters; always `None` where `/proc` does not
/// exist, so the reads metric degrades to `unavailable`.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn read_io_counters() -> Option<IoCounters> {
    None
}

/// Express an `rchar` delta in whole state files: the number of full-file
/// reads per `update`. Zero samples/size yields 0.0 rather than a NaN.
#[must_use]
pub fn reads_per_update(rchar_delta: u64, state_bytes: u64, samples: u64) -> f64 {
    if state_bytes == 0 || samples == 0 {
        return 0.0;
    }
    rchar_delta as f64 / (state_bytes as f64 * samples as f64)
}

/// One sample's input: which record to touch and the exact payload written.
fn sample_input(records: usize, sample: u32) -> Result<(String, RawRecord)> {
    let index = usize::try_from(sample)
        .map_err(|_| BenchmarkError::RecordStore("sample index overflow".to_string()))?
        % records;
    touched_record(index, i64::from(sample)).map(|record| (record.url.clone(), record))
}

/// A `DISCOVERED` record for page `index`, stamped with the fixed clock.
fn touched_record(index: usize, sample: i64) -> Result<RawRecord> {
    let url = format!("http://example.com/page/{index}");
    let mut record = RawRecord::new_discovered(&url, &url, RUN_ID, BASE_CLOCK_MS + index as i64)
        .map_err(store_error)?;
    record.updated_at = BASE_CLOCK_MS + TOUCHED_CLOCK_OFFSET_MS + sample;
    Ok(record)
}

/// Write the `records`-record seed in one atomic transaction (outside the
/// measured window).
fn seed(store: &RecordStore, records: usize) -> Result<()> {
    let plan = (0..records)
        .map(|index| touched_record(index, 0))
        .collect::<Result<Vec<_>>>()?;
    store
        .update(&mut |current: &mut DomainRecords| {
            for record in &plan {
                current.insert(record.url.clone(), record.clone());
            }
            Ok(())
        })
        .map_err(store_error)
}

/// Prove the measured samples persisted: every seeded record is still there
/// and the touched one carries the last sample's stamp. A loop that did
/// nothing would be fast AND empty, which this rejects.
fn verify(store: &RecordStore, records: usize, samples: u32) -> Result<()> {
    let persisted = store.load().map_err(store_error)?;
    if persisted.len() != records {
        return Err(BenchmarkError::RecordStore(format!(
            "post-run store holds {} records, expected {records}: a sample did not persist",
            persisted.len()
        )));
    }
    let touched = usize::try_from(samples - 1)
        .map_err(|_| BenchmarkError::RecordStore("sample index overflow".to_string()))?
        % records;
    let expected = BASE_CLOCK_MS + TOUCHED_CLOCK_OFFSET_MS + i64::from(samples - 1);
    let url = format!("http://example.com/page/{touched}");
    match persisted.get(&url).map(|record| record.updated_at) {
        Some(found) if found == expected => Ok(()),
        other => Err(BenchmarkError::RecordStore(format!(
            "post-run record {url} has updated_at {other:?}, expected {expected}: \
             the measured samples did not persist"
        ))),
    }
}

/// Translate the counter snapshots into the report's read metrics. A missing
/// snapshot (no `/proc/self/io`) leaves all three `None`.
fn read_metrics(
    before: Option<IoCounters>,
    after: Option<IoCounters>,
    state_bytes: u64,
    samples: u32,
) -> (Option<f64>, Option<u64>, Option<u64>) {
    let (Some(before), Some(after)) = (before, after) else {
        return (None, None, None);
    };
    let rchar = after.rchar.saturating_sub(before.rchar);
    let read_bytes = after.read_bytes.saturating_sub(before.read_bytes);
    let sample_count = u64::from(samples);
    (
        Some(reads_per_update(rchar, state_bytes, sample_count)),
        Some(rchar / sample_count),
        Some(read_bytes / sample_count),
    )
}

/// One error funnel for the record store (the crate's single typed surface).
fn store_error(error: RecordStoreError) -> BenchmarkError {
    BenchmarkError::RecordStore(error.to_string())
}
