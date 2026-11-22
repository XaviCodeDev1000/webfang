use super::*;
use tempfile::TempDir;

/// #1121 flush truth: a failing flush must mark the writer errored and
/// must NOT index the record — the old `let _ = file.flush()` lied about
/// durability by acknowledging a record the OS never accepted. The bytes
/// were still consumed by `write_all` before the flush failed, so the
/// offset must resync to the sink's real size, not stay at 0.
#[test]
fn append_record_flush_error_marks_write_error_and_skips_index() {
    struct FlushFails {
        written: u64,
    }
    impl Write for FlushFails {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("simulated disk full"))
        }
    }
    impl AppendSink for FlushFails {
        fn size(&self) -> Option<u64> {
            Some(self.written)
        }
    }

    let dir = TempDir::new().unwrap();
    let (_tx, rx) = mpsc::channel(1);
    let index = Arc::new(DashMap::new());
    let write_error = Arc::new(AtomicBool::new(false));
    let writer = BackgroundWriter::new(
        dir.path().join("unused.bin"),
        rx,
        Arc::clone(&index),
        Arc::clone(&write_error),
    );

    let mut offset = 0u64;
    writer.append_record(
        &mut FlushFails { written: 0 },
        &mut offset,
        "https://x.example.com".into(),
        b"payload",
    );

    assert!(
        write_error.load(Ordering::Relaxed),
        "flush failure must mark the writer errored (never `let _ =`)"
    );
    assert!(
        index.is_empty(),
        "a record whose flush failed must not be indexed as durable"
    );
    assert_eq!(
        offset,
        4 + 7 + 1,
        "offset must resync to the bytes the sink actually consumed"
    );
}

/// #1143 BLOCKER regression: a frame that dies mid-`write_all` (some
/// bytes already on the sink, then an error) must NOT leave the tracked
/// offset behind the real EOF — otherwise every later append indexes at
/// the wrong byte range forever. The writer loop keeps processing
/// commands already queued when the failure happens, so this test
/// asserts the NEXT frame lands at the resynced EOF. Fully deterministic:
/// an in-memory sink with a byte quota, zero sleeps, zero timing.
#[test]
fn append_record_resyncs_offset_after_partial_frame_failure() {
    struct TornSink {
        buf: Vec<u8>,
        quota: usize,
    }
    impl Write for TornSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.quota.min(buf.len());
            self.buf.extend_from_slice(&buf[..n]);
            self.quota -= n;
            if n == 0 {
                Err(std::io::Error::other("simulated disk full"))
            } else {
                Ok(n)
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl AppendSink for TornSink {
        fn size(&self) -> Option<u64> {
            Some(self.buf.len() as u64)
        }
    }

    let dir = TempDir::new().unwrap();
    let (_tx, rx) = mpsc::channel(1);
    let index = Arc::new(DashMap::new());
    let write_error = Arc::new(AtomicBool::new(false));
    let writer = BackgroundWriter::new(
        dir.path().join("unused.bin"),
        rx,
        Arc::clone(&index),
        Arc::clone(&write_error),
    );

    // Frame = 4 (len) + 11 (payload) + 1 (\\n) = 16 bytes; the sink
    // accepts 9 then fails — a genuinely partial frame on disk.
    let mut sink = TornSink {
        buf: Vec::new(),
        quota: 9,
    };
    let mut offset = 0u64;
    writer.append_record(
        &mut sink,
        &mut offset,
        "https://torn.example.com".into(),
        b"hello world",
    );

    assert!(write_error.load(Ordering::Relaxed));
    assert!(
        index.is_empty(),
        "the torn frame must not be indexed at any offset"
    );
    assert_eq!(
        sink.buf.len(),
        9,
        "the failure is genuinely partial — bytes ARE on the sink"
    );
    assert_eq!(
        offset, 9,
        "offset must resync to the real EOF, not stay at the stale 0"
    );

    // A later frame (still-queued command processed after the failure)
    // must land at the resynced EOF — the pre-fix arithmetic would have
    // indexed it at 0, silently corrupting reads.
    sink.quota = usize::MAX;
    writer.append_record(
        &mut sink,
        &mut offset,
        "https://after.example.com".into(),
        b"xy",
    );
    assert_eq!(
        index.get("https://after.example.com").map(|r| *r),
        Some(9),
        "post-failure frame must index at the resynced EOF"
    );
    assert_eq!(offset, 9 + 4 + 2 + 1, "offset keeps tracking real EOF");
}

/// Happy-path framing keeps the tracked offset exact across frames
/// (replaces the per-record `metadata().unwrap_or(0)` stat).
#[test]
fn append_record_tracks_offset_across_frames() {
    struct Sink(Vec<u8>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl AppendSink for Sink {
        fn size(&self) -> Option<u64> {
            Some(self.0.len() as u64)
        }
    }

    let dir = TempDir::new().unwrap();
    let (_tx, rx) = mpsc::channel(1);
    let index = Arc::new(DashMap::new());
    let write_error = Arc::new(AtomicBool::new(false));
    let writer = BackgroundWriter::new(
        dir.path().join("unused.bin"),
        rx,
        Arc::clone(&index),
        Arc::clone(&write_error),
    );

    let mut sink = Sink(Vec::new());
    let mut offset = 0u64;
    writer.append_record(
        &mut sink,
        &mut offset,
        "https://a.example.com".into(),
        b"aa",
    );
    writer.append_record(
        &mut sink,
        &mut offset,
        "https://b.example.com".into(),
        b"bbb",
    );

    assert_eq!(offset, (4 + 2 + 1) + (4 + 3 + 1), "two framed records");
    assert_eq!(index.get("https://a.example.com").map(|r| *r), Some(0u64));
    assert_eq!(index.get("https://b.example.com").map(|r| *r), Some(7u64));
    assert!(!write_error.load(Ordering::Relaxed));
}
