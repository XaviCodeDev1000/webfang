//! `bench_record_store_update` — permanent `RecordStore::update` measurement
//! (T0, issue #3081).
//!
//! One run measures the live write path of `--resume` over two seeded store
//! sizes (1,000 and 10,000 records) and prints, per size, ONE stable summary
//! line on stdout:
//!
//! ```text
//! record_store_update records=1000 samples=21 state_bytes=<n> \
//! ms_per_update_p50=<ms> ms_per_update_p95=<ms> reads_per_update=<n|unavailable> \
//! rchar_per_update=<bytes|unavailable> read_bytes_per_update=<bytes|unavailable>
//! ```
//!
//! That shape is the contract a later threshold asserts against, so a grader
//! can `grep '^record_store_update records=10000 '` and read a fixed field.
//! Latency is the house nearest-rank percentile (ADR-B3); the headline
//! `ms/update` is p50 because the median is what a threshold must compare,
//! with p95 beside it for the tail. `reads_per_update` is the `rchar` delta of
//! the measured window expressed in whole state files — the 2 full reads per
//! update that T8 removes — and is `unavailable` anywhere `/proc/self/io`
//! does not exist (Linux-only; the run still succeeds).
//!
//! Usage (release, from the suite):
//!
//! ```text
//! cargo run --release -p webfang_benchmark --bin bench_record_store_update
//! ```
//!
//! Determinism: fixed records from a fixed injected clock, a `tempfile`
//! scratch directory, no sleeps, no network, no cross-run state, and no CLI
//! arguments. Two consecutive runs differ only in the measured durations.

use std::process::ExitCode;

use webfang_benchmark::record_store_update::{measure, summary_line, DEFAULT_SAMPLES, SIZES};
use webfang_benchmark::Result;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench_record_store_update failed: {error}");
            ExitCode::FAILURE
        },
    }
}

fn run() -> Result<()> {
    // One scratch directory for the whole run; `TempDir` removes it on drop.
    let dir = tempfile::TempDir::new()?;
    for records in SIZES {
        // A fresh store per size: the 10k seed must not inherit the 1k file.
        let state_dir = dir.path().join(format!("records-{records}"));
        let report = measure(&state_dir, records, DEFAULT_SAMPLES)?;
        println!("{}", summary_line(&report));
    }
    Ok(())
}
