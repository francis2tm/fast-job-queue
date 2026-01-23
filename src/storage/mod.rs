//! Storage backend implementations for the job queue.
//!
//! This module provides:
//! - [`Storage`] trait - Push/pop interface for storage backends
//! - [`MemoryStorage`] - In-memory storage for testing

use std::future::Future;

mod memory;
#[cfg(feature = "postgres")]
mod postgres;

pub use memory::MemoryStorage;

use crate::Job;

/// Trait for job storage backends.
///
/// Both in-memory and database-backed storage implement this trait.
/// The `pop` method atomically claims the next pending job.
///
/// # Associated Types
///
/// * `Job` - The job type this storage holds
/// * `Error` - Error type for storage operations
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::{Storage, MemoryStorage};
///
/// let storage: MemoryStorage<MyJob> = MemoryStorage::new();
/// storage.push(my_job).await?;
/// if let Some(job) = storage.pop().await? {
///     job.execute(&storage).await?;
/// }
/// ```
pub trait Storage: Clone + Send + Sync + 'static {
    /// The job type this storage holds.
    type Job: Job<Self> + Send + Sync + 'static;

    /// Error type for storage operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Push a job to the queue.
    ///
    /// For database backends, this may be unimplemented if jobs
    /// are inserted via API handlers instead.
    fn push(&self, job: Self::Job) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Pop (claim) the next pending job from the queue.
    ///
    /// This should atomically:
    /// 1. Find a pending job
    /// 2. Mark it as running (to prevent other workers from claiming it)
    /// 3. Return the job data
    ///
    /// Returns `Ok(None)` if no pending jobs are available.
    fn pop(&self) -> impl Future<Output = Result<Option<Self::Job>, Self::Error>> + Send;
}
