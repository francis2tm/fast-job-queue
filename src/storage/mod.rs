//! Storage backend implementations for the job queue.
//!
//! This module provides:
//! - [`Job`] trait - Static job contract executed by queue workers
//! - [`Storage`] trait - Typed claim/wait interface for storage backends
//! - [`MemoryStorage`] - In-memory storage for testing

use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

mod erased;
mod memory;
#[cfg(feature = "postgres")]
mod postgres;

pub(crate) use erased::{StorageList, StorageListItem};
pub use memory::MemoryStorage;

/// Shared error type used by storage backends.
pub type StorageError = Box<dyn Error + Send + Sync + 'static>;

/// Outcome of attempting to execute one job from storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecuteOutcome {
    /// One job was executed (successfully or with an execution error that was returned).
    Executed,
    /// No job was available.
    Empty,
}

/// Trait implemented by jobs that can be executed by queue workers.
///
/// A storage claims one job and returns it to the queue. The queue then calls
/// this method to process the job.
pub trait Job: Send + 'static {
    /// Execute this job once.
    fn execute(self) -> impl Future<Output = Result<(), StorageError>> + Send;
}

/// Trait for queue storage backends.
///
/// Each storage is statically typed to exactly one job type via `JobType`.
///
/// # Example
///
/// ```rust,no_run
/// use fast_job_queue::{Job, MemoryStorage, Storage, StorageError};
/// use std::convert::Infallible;
///
/// struct PrintJob;
///
/// impl Job for PrintJob {
///     async fn execute(self) -> Result<(), StorageError> {
///         println!("hello");
///         Ok(())
///     }
/// }
///
/// # async fn example() -> Result<(), Infallible> {
/// let storage = MemoryStorage::new();
/// storage.job_push(PrintJob).await;
/// let job = <MemoryStorage<PrintJob> as Storage<PrintJob>>::job_pop(&storage)
///     .await
///     .expect("memory storage cannot fail");
/// assert!(job.is_some());
/// # Ok(())
/// # }
/// ```
pub trait Storage<JobType>: Send + Sync + 'static
where
    JobType: Job,
{
    /// Claim one available job.
    ///
    /// Returns `Ok(None)` if no jobs are available.
    fn job_pop(&self) -> impl Future<Output = Result<Option<JobType>, StorageError>> + Send;

    /// Wait for a job to become available or for the timeout to expire.
    ///
    /// Backends can implement event-driven notifications to wake workers quickly.
    fn wait_for_job(
        &self,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), StorageError>> + Send {
        async move {
            tokio::time::sleep(timeout).await;
            Ok(())
        }
    }
}

impl<InnerStorage, JobType> Storage<JobType> for Arc<InnerStorage>
where
    InnerStorage: Storage<JobType>,
    JobType: Job,
{
    fn job_pop(&self) -> impl Future<Output = Result<Option<JobType>, StorageError>> + Send {
        self.as_ref().job_pop()
    }

    fn wait_for_job(
        &self,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), StorageError>> + Send {
        self.as_ref().wait_for_job(timeout)
    }
}
