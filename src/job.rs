//! Job trait definition for storage-agnostic job execution.

use std::future::Future;

/// Trait for jobs that can be fetched from and executed with a storage backend.
///
/// Implement this trait for your job type, parameterized by the storage backend.
/// For Postgres jobs, use `Job<DbPool>` and leverage [`impl_diesel_fetch_and_claim!`].
///
/// # Type Parameters
///
/// * `S` - The storage backend (any `Clone + Send + Sync + 'static` type)
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::Job;
///
/// struct MyJob { id: u64 }
///
/// impl Job<MyStorage> for MyJob {
///     type Error = MyError;
///
///     async fn fetch_and_claim(storage: &MyStorage) -> Result<Option<Self>, Self::Error> {
///         // Query storage for pending job, mark as running
///         Ok(None)
///     }
///
///     async fn execute(self, storage: &MyStorage) {
///         // Perform job work, update status when done
///     }
/// }
/// ```
pub trait Job<S>: Sized + Send + Sync + 'static
where
    S: Clone + Send + Sync + 'static,
{
    /// Error type for fetch operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch a single pending job from storage and mark it as running.
    ///
    /// This method should atomically:
    /// 1. Find a pending job
    /// 2. Mark it as running (to prevent other workers from claiming it)
    /// 3. Return the job data
    ///
    /// # Returns
    ///
    /// * `Ok(Some(job))` - A job was successfully claimed
    /// * `Ok(None)` - No pending jobs available
    /// * `Err(e)` - Storage error occurred
    fn fetch_and_claim(
        storage: &S,
    ) -> impl Future<Output = Result<Option<Self>, Self::Error>> + Send;

    /// Execute the job and handle any errors internally.
    ///
    /// This method is responsible for:
    /// 1. Performing the actual job work
    /// 2. Updating the job status (completed/failed) in storage
    /// 3. Logging any errors that occur
    ///
    /// Errors should be handled internally (logged, stored in DB) rather than propagated,
    /// since workers continue processing other jobs regardless.
    fn execute(self, storage: &S) -> impl Future<Output = ()> + Send;
}
