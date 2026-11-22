//! Per-category backpressure policy for the MCP server: the concurrency
//! budget (`CategoryLimits`, tuned for the reference hardware) and the
//! semaphore registry built from it (`CategorySemaphores`). Pure policy —
//! the composed shared state that consumes it lives in the parent `state`
//! module.
//!
//! Same lock discipline as the parent: everything here is an `Arc` clone of
//! a Tokio primitive, and no `std` lock guard is ever held across an
//! `.await`.
#![deny(clippy::await_holding_lock)]

use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Per-category semaphore limits for backpressure.
/// Tuned for Intel i5-4590 (4C), 8GB DDR3, HDD.
///
/// Every permit count is a [`NonZeroUsize`] (#1132): a zero limit used to
/// be representable and `from_limits` clamped it to 1 in silence, masking
/// the misconfiguration that without the clamp deadlocks the semaphore.
/// Zero is now unnameable, so the clamp is gone and the mapping is 1:1.
#[derive(Debug)]
pub struct CategoryLimits {
    /// AI inference tools (tract-onnx, spawn_blocking heavy)
    pub ai: NonZeroUsize,
    /// HTTP scraping tools (network I/O, WAF checks)
    pub scraping: NonZeroUsize,
    /// Export tools (file I/O, serialization)
    pub export: NonZeroUsize,
    /// Obsidian vault tools (disk scan, embeddings)
    pub obsidian: NonZeroUsize,
    /// Content processing tools (CPU-bound HTML parsing)
    pub content: NonZeroUsize,
    /// URL utility tools (lightweight, string ops)
    pub url_utils: NonZeroUsize,
    /// Security tools (WAF detection, metrics)
    pub security: NonZeroUsize,
    /// Asset download tools (file I/O, network)
    pub assets: NonZeroUsize,
}

impl Default for CategoryLimits {
    fn default() -> Self {
        // The literals are non-zero by construction; the guard is the repo
        // idiom from `domain::budget` (never `expect` in production code).
        let nz = |v: usize| {
            NonZeroUsize::new(v).unwrap_or_else(|| unreachable!("limit literal {v} is non-zero"))
        };
        Self {
            ai: nz(2),         // Heavy CPU inference — limit strictly
            scraping: nz(8),   // Network I/O — can handle more concurrent
            export: nz(4),     // File I/O — moderate limit for HDD
            obsidian: nz(3),   // Disk scan + embeddings — protect vault I/O
            content: nz(6),    // CPU-bound HTML parsing — moderate
            url_utils: nz(16), // Lightweight string ops — high limit
            security: nz(8),   // WAF detection — moderate
            assets: nz(4),     // File downloads — protect HDD
        }
    }
}

/// Semaphore instances for each tool category.
#[derive(Debug)]
pub struct CategorySemaphores {
    /// Semaphore limiting concurrent AI tool calls
    pub ai: Arc<Semaphore>,
    /// Semaphore limiting concurrent scraping tool calls
    pub scraping: Arc<Semaphore>,
    /// Semaphore limiting concurrent export tool calls
    pub export: Arc<Semaphore>,
    /// Semaphore limiting concurrent Obsidian tool calls
    pub obsidian: Arc<Semaphore>,
    /// Semaphore limiting concurrent content processing tool calls
    pub content: Arc<Semaphore>,
    /// Semaphore limiting concurrent URL utility tool calls
    pub url_utils: Arc<Semaphore>,
    /// Semaphore limiting concurrent security & diagnostics tool calls
    pub security: Arc<Semaphore>,
    /// Semaphore limiting concurrent asset download tool calls
    pub assets: Arc<Semaphore>,
}

impl CategorySemaphores {
    /// Build semaphores from per-category concurrency limits.
    ///
    /// The mapping is 1:1 (#1132): the old `max(1)` clamp is gone because
    /// `CategoryLimits` fields are [`NonZeroUsize`] — a zero-permit
    /// semaphore cannot be named, so there is nothing left to clamp.
    pub fn from_limits(limits: &CategoryLimits) -> Self {
        Self {
            ai: Arc::new(Semaphore::new(limits.ai.get())),
            scraping: Arc::new(Semaphore::new(limits.scraping.get())),
            export: Arc::new(Semaphore::new(limits.export.get())),
            obsidian: Arc::new(Semaphore::new(limits.obsidian.get())),
            content: Arc::new(Semaphore::new(limits.content.get())),
            url_utils: Arc::new(Semaphore::new(limits.url_utils.get())),
            security: Arc::new(Semaphore::new(limits.security.get())),
            assets: Arc::new(Semaphore::new(limits.assets.get())),
        }
    }
}
