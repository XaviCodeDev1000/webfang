//! Byte-bounded backlog of pages a full capture channel defers to.
//!
//! The second bound of the pair that replaces #1616's unbounded producer: the
//! channel bounds pages waiting to be spooled, and `max_bytes` bounds the
//! bodies held behind a full channel. Used only by
//! [`BoundedFileSink`](super::BoundedFileSink) and the spool writer.

#![deny(clippy::await_holding_lock)]

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::application::crawler::content_sink::CapturedPage;

/// Result of offering a page to the `backlog`.
pub(super) enum Admit {
    /// Accepted; the writer has been woken.
    Queued,
    /// The byte ceiling is reached. `first` is true only on the transition, so
    /// the sink can report the trip exactly once instead of once per page.
    Saturated { first: bool },
}

/// Pages a full channel defers to, drained by the writer task.
///
/// This is the second bound of the pair that replaces #1616's unbounded
/// producer: the channel bounds pages waiting to be spooled, and `max_bytes`
/// bounds the bodies held behind a full channel. A `Mutex` is the right tool
/// because every critical section is a synchronous check-and-push with no
/// `.await` inside — the writer takes the pages out and releases the guard
/// before writing.
#[derive(Debug)]
pub(super) struct Backlog {
    inner: Mutex<BacklogState>,
    /// Wakes the writer after an admit. `Notify` stores a permit, so a push
    /// that races the writer's drain cannot be lost.
    notify: Notify,
    max_bytes: usize,
}

/// Deferred pages plus their byte accounting, guarded by one lock so the
/// admit-check and the push are atomic with respect to other workers.
#[derive(Debug, Default)]
struct BacklogState {
    pages: VecDeque<CapturedPage>,
    /// Sum of `url.len() + html.len()` over held pages.
    bytes: usize,
    /// Sticky: the byte ceiling has tripped, so later admits are refused.
    saturated: bool,
}

impl Backlog {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(BacklogState::default()),
            notify: Notify::new(),
            max_bytes,
        }
    }

    /// Byte ceiling for held pages.
    pub(super) fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Offer a page, or refuse it if the ceiling is already reached.
    pub(super) fn admit(&self, page: CapturedPage) -> Admit {
        let incoming = page.url.len().saturating_add(page.html.len());
        // A poisoned mutex (a worker panicked mid-admit) degrades to the
        // recovered backlog rather than panicking, as the in-memory sink does.
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.bytes.saturating_add(incoming) > self.max_bytes {
            let first = !state.saturated;
            state.saturated = true;
            return Admit::Saturated { first };
        }
        state.bytes = state.bytes.saturating_add(incoming);
        state.pages.push_back(page);
        drop(state);
        // Signal AFTER the push is visible under the lock, so a writer that
        // wakes cannot miss the page; and a writer that is already parked gets
        // the stored permit.
        self.notify.notify_one();
        Admit::Queued
    }

    /// Take every deferred page, leaving the backlog empty.
    pub(super) fn drain(&self) -> Vec<CapturedPage> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.bytes = 0;
        std::mem::take(&mut state.pages).into()
    }

    /// Whether no page is waiting in the backlog.
    pub(super) fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pages
            .is_empty()
    }

    /// `(held_pages, held_bytes)` currently in the backlog.
    pub(super) fn occupancy(&self) -> (usize, usize) {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.pages.len(), state.bytes)
    }

    /// Wake the writer without changing the backlog.
    pub(super) async fn notified(&self) {
        self.notify.notified().await;
    }
}
