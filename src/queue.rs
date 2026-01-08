//! Job queue implementation with configurable workers.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, error};

use crate::Job;

/// Errors that can occur when using the job queue.
#[derive(Debug, Error)]
pub enum JobQueueError {
    /// Configuration error (e.g., zero workers, zero poll interval).
    ///
    /// This error is returned when [`JobQueue::new`] is called with invalid
    /// configuration values.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// A worker panicked during execution.
    ///
    /// This error is returned during [`JobQueue::shutdown`] if a worker task
    /// panicked while processing a job.
    #[error("Worker panicked: {0}")]
    WorkerPanicked(String),
}

/// Configuration for the job queue.
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use fast_job_queue::JobQueueConfig;
///
/// // Use defaults
/// let config = JobQueueConfig::default();
///
/// // Or customize
/// let config = JobQueueConfig {
///     workers: 8,
///     poll_interval: Duration::from_millis(100),
/// };
/// ```
#[derive(Clone, Debug)]
pub struct JobQueueConfig {
    /// Number of concurrent worker tasks.
    ///
    /// Each worker independently polls for and processes jobs.
    /// More workers allow higher throughput but consume more resources.
    pub workers: usize,

    /// How long to wait between polls when no jobs are available.
    ///
    /// Lower values mean faster response to new jobs but higher CPU usage.
    /// Higher values reduce CPU usage but increase latency for new jobs.
    pub poll_interval: Duration,
}

impl Default for JobQueueConfig {
    /// Returns a configuration with sensible defaults.
    ///
    /// - `workers`: 4
    /// - `poll_interval`: 1 second
    fn default() -> Self {
        Self {
            workers: 4,
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// A storage-agnostic async job queue with configurable workers.
///
/// The job queue spawns multiple worker tasks that poll for jobs from a storage
/// backend and execute them concurrently.
///
/// # Type Parameters
///
/// When creating a queue with [`JobQueue::new`], you specify:
/// - `J` - The job type implementing [`Job<S>`]
/// - `S` - The storage backend implementing [`Storage`]
///
/// # Lifecycle
///
/// 1. Create with [`JobQueue::new`]
/// 2. Workers automatically start polling and processing jobs
/// 3. Call [`JobQueue::shutdown`] to gracefully stop all workers
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::{JobQueue, JobQueueConfig};
///
/// let queue = JobQueue::new::<MyJob, MyStorage>(storage, JobQueueConfig::default())?;
///
/// // Queue processes jobs in the background...
///
/// queue.shutdown().await?;
/// ```
pub struct JobQueue {
    workers: Vec<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl JobQueue {
    /// Create a new job queue that processes jobs of type `J` using storage `S`.
    ///
    /// This spawns `config.workers` background tasks that poll for jobs.
    ///
    /// # Errors
    ///
    /// Returns [`JobQueueError::InvalidConfig`] if:
    /// - `workers` is 0
    /// - `poll_interval` is 0
    #[must_use = "job queue must be stored to keep workers running"]
    pub fn new<J, S>(storage: S, config: JobQueueConfig) -> Result<Self, JobQueueError>
    where
        J: Job<S>,
        S: Clone + Send + Sync + 'static,
    {
        if config.workers == 0 {
            return Err(JobQueueError::InvalidConfig(
                "workers must be greater than 0".into(),
            ));
        }

        if config.poll_interval == Duration::from_millis(0) {
            return Err(JobQueueError::InvalidConfig(
                "poll_interval must be greater than 0".into(),
            ));
        }

        let (shutdown_tx, _) = broadcast::channel(1);

        let mut workers = Vec::with_capacity(config.workers);
        for worker_id in 0..config.workers {
            let storage = storage.clone();
            let config = config.clone();
            let mut shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                debug!(worker_id = worker_id, "Worker starting");

                loop {
                    // Check for shutdown signal
                    if shutdown_rx.try_recv().is_ok() {
                        debug!(worker_id = worker_id, "Worker received shutdown signal");
                        break;
                    }

                    // Fetch and claim a single job from storage
                    match J::fetch_and_claim(&storage).await {
                        Ok(Some(job)) => {
                            debug!(worker_id = worker_id, "Worker claimed job");

                            // Execute the job
                            job.execute(&storage).await;
                        }
                        Ok(None) => {
                            // No jobs available, wait before polling again
                            tokio::time::sleep(config.poll_interval).await;
                        }
                        Err(e) => {
                            error!(
                                worker_id = worker_id,
                                error = %e,
                                "Failed to fetch jobs"
                            );
                            tokio::time::sleep(config.poll_interval).await;
                        }
                    }
                }

                debug!(worker_id = worker_id, "Worker shutting down");
            });

            workers.push(handle);
        }

        Ok(Self {
            workers,
            shutdown_tx,
        })
    }

    /// Gracefully shutdown the queue.
    ///
    /// This signals all workers to stop and waits for them to finish processing
    /// their current jobs.
    ///
    /// # Errors
    ///
    /// Returns [`JobQueueError::WorkerPanicked`] if any worker panicked during
    /// execution.
    pub async fn shutdown(self) -> Result<(), JobQueueError> {
        // Send shutdown signal to all workers
        let _ = self.shutdown_tx.send(());

        // Wait for all workers to complete
        for (idx, handle) in self.workers.into_iter().enumerate() {
            handle.await.map_err(|e| {
                JobQueueError::WorkerPanicked(format!("Worker {} panicked: {}", idx, e))
            })?;
        }

        tracing::info!("All workers shut down successfully");
        Ok(())
    }

    /// Attempt to shutdown an `Arc<JobQueue>` if it's the last reference.
    ///
    /// This is useful for graceful shutdown in application cleanup code where
    /// the queue may still have active references.
    ///
    /// # Behavior
    ///
    /// - If this is the last `Arc` reference, shutdowns the queue and awaits completion
    /// - If other references exist, logs a warning and returns without shutdown
    pub async fn shutdown_arc(self: Arc<Self>) {
        tracing::info!("Shutting down job queue...");
        match Arc::try_unwrap(self) {
            Ok(queue) => {
                if let Err(e) = queue.shutdown().await {
                    tracing::error!("Error shutting down job queue: {:?}", e);
                } else {
                    tracing::info!("Job queue shut down successfully");
                }
            }
            Err(_) => {
                tracing::warn!("Could not shutdown job queue - still has active references");
            }
        }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Job, MemoryStorage};
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn config_default_has_sensible_values() {
        let config = JobQueueConfig::default();
        assert_eq!(config.workers, 4);
        assert_eq!(config.poll_interval, Duration::from_secs(1));
    }

    #[test]
    fn config_clone_works() {
        let config = JobQueueConfig {
            workers: 10,
            poll_interval: Duration::from_millis(500),
        };
        let cloned = config.clone();
        assert_eq!(cloned.workers, 10);
        assert_eq!(cloned.poll_interval, Duration::from_millis(500));
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    // Simple job type for validation tests (doesn't need counter)
    #[derive(Debug, Clone)]
    struct SimpleJob;

    impl Job<MemoryStorage<SimpleJob>> for SimpleJob {
        type Error = Infallible;

        async fn fetch_and_claim(
            storage: &MemoryStorage<SimpleJob>,
        ) -> Result<Option<Self>, Infallible> {
            Ok(storage.pop().await)
        }

        async fn execute(self, _storage: &MemoryStorage<SimpleJob>) {}
    }

    #[tokio::test]
    async fn new_rejects_zero_workers() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();
        let config = JobQueueConfig {
            workers: 0,
            poll_interval: Duration::from_millis(100),
        };

        let result = JobQueue::new::<SimpleJob, _>(storage, config);
        match result {
            Err(e) => assert!(e.to_string().contains("workers must be greater than 0")),
            Ok(_) => panic!("Expected error for zero workers"),
        }
    }

    #[tokio::test]
    async fn new_rejects_zero_poll_interval() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();
        let config = JobQueueConfig {
            workers: 2,
            poll_interval: Duration::from_millis(0),
        };

        let result = JobQueue::new::<SimpleJob, _>(storage, config);
        match result {
            Err(e) => assert!(
                e.to_string()
                    .contains("poll_interval must be greater than 0")
            ),
            Ok(_) => panic!("Expected error for zero poll_interval"),
        }
    }

    // =========================================================================
    // JobQueue Behavior Tests
    // =========================================================================

    // Storage wrapper that includes a counter for tracking executed jobs
    #[derive(Clone)]
    struct CountingStorage {
        jobs: MemoryStorage<u64>,
        executed: Arc<AtomicUsize>,
    }

    impl CountingStorage {
        fn new() -> Self {
            Self {
                jobs: MemoryStorage::new(),
                executed: Arc::new(AtomicUsize::new(0)),
            }
        }

        async fn push(&self, job: u64) {
            self.jobs.push(job).await;
        }

        fn count(&self) -> usize {
            self.executed.load(Ordering::SeqCst)
        }

        async fn is_empty(&self) -> bool {
            self.jobs.is_empty().await
        }
    }

    // Job type that increments counter on execute
    struct CountingJob {
        _id: u64,
        counter: Arc<AtomicUsize>,
    }

    impl Job<CountingStorage> for CountingJob {
        type Error = Infallible;

        async fn fetch_and_claim(storage: &CountingStorage) -> Result<Option<Self>, Infallible> {
            Ok(storage.jobs.pop().await.map(|id| CountingJob {
                _id: id,
                counter: storage.executed.clone(),
            }))
        }

        async fn execute(self, _storage: &CountingStorage) {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn processes_single_job() {
        let storage = CountingStorage::new();
        storage.push(1).await;

        let config = JobQueueConfig {
            workers: 1,
            poll_interval: Duration::from_millis(10),
        };

        let queue = JobQueue::new::<CountingJob, _>(storage.clone(), config).unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        queue.shutdown().await.unwrap();

        assert_eq!(storage.count(), 1);
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn processes_multiple_jobs() {
        let storage = CountingStorage::new();
        for i in 0..5 {
            storage.push(i).await;
        }

        let config = JobQueueConfig {
            workers: 2,
            poll_interval: Duration::from_millis(10),
        };

        let queue = JobQueue::new::<CountingJob, _>(storage.clone(), config).unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        queue.shutdown().await.unwrap();

        assert_eq!(storage.count(), 5);
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn shutdown_is_graceful() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let config = JobQueueConfig {
            workers: 2,
            poll_interval: Duration::from_millis(10),
        };

        let queue = JobQueue::new::<SimpleJob, _>(storage, config).unwrap();

        let result = queue.shutdown().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn idle_workers_respect_poll_interval() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let config = JobQueueConfig {
            workers: 1,
            poll_interval: Duration::from_millis(50),
        };

        let queue = JobQueue::new::<SimpleJob, _>(storage, config).unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        queue.shutdown().await.unwrap();
    }
}
