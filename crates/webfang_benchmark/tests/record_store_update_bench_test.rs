#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! T0 (#3081) — the permanent `RecordStore::update` measurement.
//!
//! What is pinned here: the summary line a later threshold (T8) asserts
//! against, the derivation of `reads/update` from `/proc/self/io` deltas
//! (L29), and the fact that the measured loop really persists records instead
//! of being optimized into nothing.
//!
//! Since T8 `update` reads the state file exactly ONCE per call: the load that
//! feeds `mutate` is also the read that carries the envelope's `created_at`
//! forward, so the writer never has to re-read the file it just parsed.
//!
//! The timing itself is NOT asserted: wall-clock numbers are machine state,
//! not behavior. The tests exercise the arithmetic, the line format and the
//! persistence side effect.
//!
//! Run with: cargo nextest run -p webfang_benchmark --test record_store_update_bench_test

use tempfile::TempDir;

use webfang_benchmark::record_store_update::{
    measure, parse_proc_io, reads_per_update, summary_line, IoCounters, UpdateReport, DOMAIN_KEY,
};
use webfang_core::infrastructure::export::RecordStore;

/// The two sizes the benchmark reports (requirement 1).
const REPORTED_SIZES: [usize; 2] = [1_000, 10_000];

/// Updates one exported page costs: a commit marks the page in flight and then
/// marks it committed (S14). The per-commit read count is therefore the
/// per-update count doubled — 2 after T8, 4 before it.
const UPDATES_PER_COMMIT: u32 = 2;

/// The precision `reads_per_update` is reported with, and therefore the
/// precision the structural assertion is stated at.
///
/// A window cannot be bit-exact: besides the state file, the process reads a
/// fixed ~20 bytes per update of unrelated bytes (measured 19.8 in debug and
/// 6.0 in release), so the raw ratio is always `1 + ~20 / state_bytes`. That
/// term does NOT scale with the file size or with the number of reads, and the
/// fixture below is large enough to keep it under the third decimal — a second
/// read of the state file would add a whole `1.000`, not a thousandth.
const REPORTED_READS_SCALE: f64 = 1_000.0;

/// A `/proc/self/io` snapshot with all six documented counters, in kernel
/// order, so parsing depends on the field names, never on line position.
const PROC_IO_SAMPLE: &str = "\
rchar: 123456
wchar: 789
syscr: 42
syscw: 7
read_bytes: 4096
write_bytes: 8192
cancelled_write_bytes: 0
";

fn report(reads: Option<f64>, rchar: Option<u64>, read_bytes: Option<u64>) -> UpdateReport {
    UpdateReport {
        records: 1_000,
        samples: 21,
        state_bytes: 409_600,
        ms_p50: 0.125,
        ms_p95: 0.5,
        reads_per_update: reads,
        rchar_per_update: rchar,
        read_bytes_per_update: read_bytes,
    }
}

/// The two sizes the binary reports are the ones the plan names, so a later
/// threshold (T8) can grep a known line per size.
#[test]
fn reported_sizes_are_one_and_ten_thousand() {
    assert_eq!(REPORTED_SIZES, [1_000, 10_000]);
}

/// `/proc/self/io` parsing reads the two counters the plan names by name.
#[test]
fn parse_proc_io_extracts_rchar_and_read_bytes() {
    let counters = parse_proc_io(PROC_IO_SAMPLE).expect("all counters present");
    assert_eq!(
        counters,
        IoCounters {
            rchar: 123_456,
            read_bytes: 4_096,
        }
    );
}

/// A counter the kernel omits is a missing metric, never a zero: reporting
/// `reads/update = 0` would look like a free update.
#[test]
fn parse_proc_io_rejects_a_missing_counter() {
    assert_eq!(parse_proc_io("rchar: 10\n"), None);
    assert_eq!(parse_proc_io("read_bytes: 10\n"), None);
    assert_eq!(parse_proc_io("rchar: ten\nread_bytes: 10\n"), None);
    assert_eq!(parse_proc_io(""), None);
}

/// `reads/update` is the rchar delta expressed in whole state files: since T8
/// `update` reads the file once (the load doubles as the `created_at` source),
/// so one state file per sample means 1 read.
#[test]
fn reads_per_update_normalizes_the_delta_by_state_file_size() {
    // 10 samples, each reading a 1000-byte file twice.
    assert!((reads_per_update(20_000, 1_000, 10) - 2.0).abs() < 1e-12);
    assert!((reads_per_update(10_000, 1_000, 10) - 1.0).abs() < 1e-12);
    // A zero-size state file cannot normalize anything.
    assert!((reads_per_update(20_000, 0, 10) - 0.0).abs() < f64::EPSILON);
}

/// The summary line is the contract T8 asserts against: one stable,
/// greppable, whitespace-free-field line per size, and the metric names carry
/// their unit.
#[test]
fn summary_line_is_stable_and_greppable() {
    assert_eq!(
        summary_line(&report(Some(1.0), Some(409_600), Some(0))),
        "record_store_update records=1000 samples=21 state_bytes=409600 \
         ms_per_update_p50=0.125 ms_per_update_p95=0.500 reads_per_update=1.000 \
         rchar_per_update=409600 read_bytes_per_update=0"
    );
}

/// Off Linux the reads metric degrades to the literal `unavailable` and the
/// line keeps its shape: the run still succeeds (requirement 3).
#[test]
fn summary_line_reports_unavailable_off_linux() {
    assert_eq!(
        summary_line(&report(None, None, None)),
        "record_store_update records=1000 samples=21 state_bytes=409600 \
         ms_per_update_p50=0.125 ms_per_update_p95=0.500 reads_per_update=unavailable \
         rchar_per_update=unavailable read_bytes_per_update=unavailable"
    );
}

/// The measured loop must do real work: every sample drives one atomic
/// read-modify-write, so the store holds exactly the seeded records afterwards
/// and the last sample's write is on disk.
#[test]
fn measure_persists_every_sample_and_reports_percentiles() {
    let dir = TempDir::new().unwrap();
    let result = measure(dir.path(), 64, 5).expect("measurement succeeds");

    assert_eq!(result.records, 64);
    assert_eq!(result.samples, 5);
    assert!(result.state_bytes > 0, "the seeded store must be on disk");
    assert!(
        result.ms_p50 > 0.0 && result.ms_p95 >= result.ms_p50,
        "nearest-rank percentiles over real samples: {result:?}"
    );

    let store = RecordStore::new(DOMAIN_KEY).with_state_dir(dir.path().to_path_buf());
    assert_eq!(
        store.load().expect("store readable").len(),
        64,
        "the measured samples must not change the record count"
    );
}

/// The structural invariant T8 delivers, measured by the kernel: `update`
/// reads the state file exactly ONCE per call, so `rchar` advances by one state
/// file per sample plus the fixed harness residual — reported as exactly
/// `1.000`. A second read (the `save_locked` → `existing_created_at` one T8
/// removes) lands here as a `2.000`.
#[cfg(target_os = "linux")]
#[test]
fn measure_reports_one_full_file_read_per_update_on_linux() {
    // 256 records, ~87 KiB of state: large enough that the fixed per-update
    // residual stays below the reported precision (see REPORTED_READS_SCALE).
    let dir = TempDir::new().unwrap();
    let result = measure(dir.path(), 256, 5).expect("measurement succeeds");

    let reads = result
        .reads_per_update
        .expect("/proc/self/io is available on Linux");
    assert!(reads >= 1.0, "rchar must explain a full read: {result:?}");
    assert_eq!(
        (reads * REPORTED_READS_SCALE).round() / REPORTED_READS_SCALE,
        1.0,
        "update must read the state file exactly once per call (reported as 1.000), \
         got {reads} reads/update ({result:?})"
    );
    assert_eq!(
        (reads * f64::from(UPDATES_PER_COMMIT) * REPORTED_READS_SCALE).round()
            / REPORTED_READS_SCALE,
        2.0,
        "a commit is two updates, so it must read the file twice (S14): {result:?}"
    );
}

/// The same guarantee stated the other way: where `/proc/self/io` does not
/// exist the metric is `None` and the measurement still succeeds.
#[cfg(not(target_os = "linux"))]
#[test]
fn measure_reports_unavailable_off_linux() {
    let dir = TempDir::new().unwrap();
    let result = measure(dir.path(), 64, 5).expect("measurement succeeds");
    assert_eq!(result.reads_per_update, None);
    assert_eq!(result.rchar_per_update, None);
    assert_eq!(result.read_bytes_per_update, None);
}

/// Both reported sizes must be measurable with no hidden special case.
#[test]
fn measure_handles_both_reported_sizes() {
    for records in REPORTED_SIZES {
        let dir = TempDir::new().unwrap();
        let result = measure(dir.path(), records, 1).expect("measurement succeeds");
        assert_eq!(result.records, records);
        assert!(summary_line(&result)
            .starts_with(&format!("record_store_update records={records} samples=1 ")));
    }
}
