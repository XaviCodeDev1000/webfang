//! SQLite schema, WAL pragmas, and connection pool for elastic ingestion.
//!
//! Schema is frozen per `design.md` (the authoritative frozen artifact). The
//! task-list DDL transcription differed in three places; this implementation
//! follows the frozen design:
//! - `chunks.id` is `TEXT` (required by `VectorRepository::save_vector(id: &str)`
//!   in PR4 — the task's `INTEGER` would force a destructive migration).
//! - `created_at` is `TEXT` (SQLite stores timestamps as TEXT regardless; the
//!   task's `TIMESTAMP` is cosmetic but diverges from design).
//! - the `content_hash` index lives on `resources` (which has the column); the
//!   task's `idx_chunks_hash ON chunks(content_hash)` referenced a non-existent
//!   column and would fail `setup_schema`.

mod blob;
mod connection;
mod note_repository;
mod retention;
mod schema;
mod vector_repository;

pub use connection::{create_memory_pool, create_pool};
pub use retention::prune_older_than;
pub use schema::SCHEMA_VERSION;
pub use vector_repository::{setup_schema, SqliteVectorRepository};

pub(crate) use schema::enforce_schema_version;

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
