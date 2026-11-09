//! Minimal first-party SQLite pool, adapted from `deadpool-sqlite 0.8.1`.
//!
//! Upstream `deadpool-sqlite 0.14` pins `rusqlite ^0.40` and requires Rust 1.95,
//! so it cannot be used with `rusqlite 0.41` on this workspace's MSRV. This
//! module keeps the same `deadpool` manager contract and the same public type
//! aliases, so call sites only change the import path.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use deadpool::managed::{self, RecycleError};
use deadpool_sync::SyncWrapper;

pub use deadpool::managed::reexports::*;
pub use deadpool_sync::reexports::*;
pub use rusqlite;

deadpool::managed_reexports!(
    "rusqlite",
    Manager,
    managed::Object<Manager>,
    rusqlite::Error,
    std::convert::Infallible
);

/// Configuration for the SQLite pool.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Path to the SQLite database file.
    pub path: PathBuf,
}

impl Config {
    /// Create a config for the given database path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

/// [`Manager`] for creating and recycling SQLite [`Connection`]s.
#[derive(Debug)]
pub struct Manager {
    config: Config,
    recycle_count: AtomicUsize,
    runtime: Runtime,
}

impl Manager {
    /// Create a manager from a config and a deadpool runtime.
    #[must_use]
    pub fn from_config(config: &Config, runtime: Runtime) -> Self {
        Self {
            config: config.clone(),
            recycle_count: AtomicUsize::new(0),
            runtime,
        }
    }
}

impl managed::Manager for Manager {
    type Type = SyncWrapper<rusqlite::Connection>;
    type Error = rusqlite::Error;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let path = self.config.path.clone();
        SyncWrapper::new(self.runtime, move || rusqlite::Connection::open(path)).await
    }

    async fn recycle(
        &self,
        conn: &mut Self::Type,
        _: &Metrics,
    ) -> managed::RecycleResult<Self::Error> {
        if conn.is_mutex_poisoned() {
            return Err(RecycleError::Message(
                "Mutex is poisoned. Connection is considered unusable.".into(),
            ));
        }
        let recycle_count = self.recycle_count.fetch_add(1, Ordering::Relaxed);
        // `rusqlite 0.41` no longer implements `ToSql` for `usize`/`u64`.
        let expected = recycle_count as i64;
        let n: i64 = conn
            .interact(move |conn| conn.query_row("SELECT $1", [expected], |row| row.get(0)))
            .await
            .map_err(|e| RecycleError::message(format!("{}", e)))??;
        if n == expected {
            Ok(())
        } else {
            Err(RecycleError::message("Recycle count mismatch"))
        }
    }
}

/// Type alias for [`Object`], matching `deadpool-sqlite::Connection`.
pub type Connection = Object;
