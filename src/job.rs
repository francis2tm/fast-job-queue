//! Job trait definition for executable jobs.

use std::future::Future;

/// Trait for jobs that can be executed with a storage backend.
///
/// The job receives access to the storage for status updates or other operations.
///
/// # Type Parameters
///
/// * `S` - The storage backend type
///
/// # Example
///
/// ```rust,no_run
/// use fast_job_queue::Job;
/// use std::convert::Infallible;
///
/// struct MyStorage;
/// struct MyJob { id: u64 }
///
/// impl Job<MyStorage> for MyJob {
///     type Error = Infallible;
///
///     async fn execute(self, _storage: &MyStorage) -> Result<(), Self::Error> {
///         println!("Processing job {}", self.id);
///         Ok(())
///     }
/// }
/// ```
pub trait Job<S>
where
    S: Send + Sync,
{
    /// Error type for execute operations.
    ///
    /// Use `std::convert::Infallible` if errors are handled internally.
    type Error: std::error::Error + Send + Sync;

    /// Execute the job.
    ///
    /// This method is responsible for:
    /// 1. Performing the actual job work
    /// 2. Updating the job status (completed/failed) in storage if needed
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Job completed successfully
    /// * `Err(e)` - Job failed; error will be logged by the queue
    fn execute(self, storage: &S) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
