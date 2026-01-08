//! Storage backend implementations for the job queue.
//!
//! This module provides:
//! - [`MemoryStorage`] - In-memory storage for testing
//! - `DbPool` (with `postgres` feature) - Postgres via diesel

mod memory;
#[cfg(feature = "postgres")]
mod postgres;

pub use memory::MemoryStorage;
