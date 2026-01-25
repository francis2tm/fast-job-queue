//! Job queue implementation with configurable workers.

use crate::{Job, Storage};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, error};

/// Errors that can occur when using the job queue.
#[derive(Debug, Error)]
pub enum JobQueueError {
    /// Configuration error (e.g., zero workers, zero poll interval).
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// A worker panicked during execution.
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
    pub workers: usize,

    /// How long to wait between polls when no jobs are available.
    pub poll_interval: Duration,
}

impl Default for JobQueueConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// A storage-backed async job queue with configurable workers.
///
/// The job queue spawns multiple worker tasks that poll for jobs from storage
/// and execute them concurrently.
///
/// # Type Parameters
///
/// * `S` - The storage backend implementing [`Storage`]
///
/// # Lifecycle
///
/// 1. Create with [`JobQueue::new`]
/// 2. Workers automatically start polling and processing jobs via `storage.pop()`
/// 3. Call [`JobQueue::shutdown`] to gracefully stop all workers
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::{JobQueue, JobQueueConfig, MemoryStorage};
///
/// let storage = MemoryStorage::new();
/// let queue = JobQueue::new(JobQueueConfig::default(), storage)?;
///
/// // Queue processes jobs in the background...
///
/// queue.shutdown().await?;
/// ```
#[derive(Clone)]
pub struct JobQueue<S: Storage> {
    inner: Arc<JobQueueInner<S>>,
}

struct JobQueueInner<S: Storage> {
    storage: S,
    workers: Mutex<Vec<JoinHandle<()>>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl<S: Storage> JobQueue<S> {
    /// Create a new job queue with the given storage backend.
    ///
    /// This spawns `config.workers` background tasks that poll for jobs.
    ///
    /// # Errors
    ///
    /// Returns [`JobQueueError::InvalidConfig`] if:
    /// - `workers` is 0
    /// - `poll_interval` is 0
    #[must_use = "job queue must be stored to keep workers running"]
    pub fn new(config: JobQueueConfig, storage: S) -> Result<Self, JobQueueError> {
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

                    // Pop (claim) the next pending job from storage
                    match storage.pop().await {
                        Ok(Some(job)) => {
                            debug!(worker_id = worker_id, "Worker claimed job");

                            // Execute the job and log any errors
                            match job.execute(&storage).await {
                                Ok(()) => {
                                    debug!(worker_id = worker_id, "Job completed successfully");
                                }
                                Err(e) => {
                                    error!(
                                        worker_id = worker_id,
                                        error = %e,
                                        "Job execution failed"
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            // No jobs available, wait for jobs OR shutdown signal
                            tokio::select! {
                                res = storage.wait_for_job(config.poll_interval) => {
                                    if let Err(e) = res {
                                        error!(
                                            worker_id = worker_id,
                                            error = %e,
                                            "Failed to wait for job"
                                        );
                                        // Fallback to sleep if waiting fails
                                        tokio::time::sleep(config.poll_interval).await;
                                    }
                                }
                                _ = shutdown_rx.recv() => {
                                    debug!(worker_id = worker_id, "Worker received shutdown signal while waiting");
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                worker_id = worker_id,
                                error = %e,
                                "Failed to pop job from storage"
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
            inner: Arc::new(JobQueueInner {
                storage,
                workers: Mutex::new(workers),
                shutdown_tx,
            }),
        })
    }

    /// Get a reference to the storage backend.
    pub fn storage(&self) -> &S {
        &self.inner.storage
    }

    /// Gracefully shutdown the queue.
    ///
    /// This signals all workers to stop and waits for them to finish processing
    /// their current jobs.
    pub async fn shutdown(&self) -> Result<(), JobQueueError> {
        let _ = self.inner.shutdown_tx.send(());
        let mut workers = self.inner.workers.lock().await;

        for (idx, handle) in workers.drain(..).enumerate() {
            handle.await.map_err(|e| {
                JobQueueError::WorkerPanicked(format!("Worker {} panicked: {}", idx, e))
            })?;
        }

        tracing::info!("All workers shut down successfully");
        Ok(())
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Job, MemoryStorage, Storage};
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

    #[derive(Debug, Clone)]
    struct SimpleJob;

    impl Job<MemoryStorage<SimpleJob>> for SimpleJob {
        type Error = Infallible;

        async fn execute(self, _storage: &MemoryStorage<SimpleJob>) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn new_rejects_zero_workers() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();
        let config = JobQueueConfig {
            workers: 0,
            poll_interval: Duration::from_millis(100),
        };

        let result = JobQueue::new(config, storage);
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

        let result = JobQueue::new(config, storage);
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

        fn count(&self) -> usize {
            self.executed.load(Ordering::SeqCst)
        }

        async fn is_empty(&self) -> bool {
            self.jobs.is_empty().await
        }
    }

    struct CountingJob {
        _id: u64,
        counter: Arc<AtomicUsize>,
    }

    impl Job<CountingStorage> for CountingJob {
        type Error = Infallible;

        async fn execute(self, _storage: &CountingStorage) -> Result<(), Infallible> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl Storage for CountingStorage {
        type Job = CountingJob;
        type Error = Infallible;

        async fn push(&self, _job: Self::Job) -> Result<(), Self::Error> {
            // Not used in these tests
            Ok(())
        }

        async fn pop(&self) -> Result<Option<Self::Job>, Self::Error> {
            Ok(self.jobs.pop().await.ok().flatten().map(|id| CountingJob {
                _id: id,
                counter: self.executed.clone(),
            }))
        }
    }

    // Helper to push raw job IDs to CountingStorage
    impl CountingStorage {
        async fn push_id(&self, id: u64) {
            self.jobs.push(id).await.unwrap();
        }
    }

    // Need a simple impl of Job for u64 so MemoryStorage<u64> works
    impl Job<MemoryStorage<u64>> for u64 {
        type Error = Infallible;

        async fn execute(self, _storage: &MemoryStorage<u64>) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn processes_single_job() {
        let storage = CountingStorage::new();
        storage.push_id(1).await;

        let config = JobQueueConfig {
            workers: 1,
            poll_interval: Duration::from_millis(10),
        };

        let queue = JobQueue::new(config, storage.clone()).unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        queue.shutdown().await.unwrap();

        assert_eq!(storage.count(), 1);
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn processes_multiple_jobs() {
        let storage = CountingStorage::new();
        for i in 0..5 {
            storage.push_id(i).await;
        }

        let config = JobQueueConfig {
            workers: 2,
            poll_interval: Duration::from_millis(10),
        };

        let queue = JobQueue::new(config, storage.clone()).unwrap();

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

        let queue = JobQueue::new(config, storage).unwrap();

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

        let queue = JobQueue::new(config, storage).unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn queue_clone_shares_state() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();
        let config = JobQueueConfig {
            workers: 1,
            poll_interval: Duration::from_millis(10),
        };

        let queue1 = JobQueue::new(config, storage).unwrap();
        let queue2 = queue1.clone();

        // Shutdown through one handle should stop workers for both
        queue1.shutdown().await.unwrap();

        // Second shutdown should complete immediately (workers already stopped)
        // or just return ok
        queue2.shutdown().await.unwrap();
    }

    struct NotifyJob {
        notify: Arc<tokio::sync::Notify>,
    }

    impl Job<MemoryStorage<NotifyJob>> for NotifyJob {
        type Error = Infallible;
        async fn execute(self, _storage: &MemoryStorage<NotifyJob>) -> Result<(), Infallible> {
            self.notify.notify_one();
            Ok(())
        }
    }

    #[tokio::test]
    async fn job_queue_wait_for_job_processes_instantly() {
        let storage: MemoryStorage<NotifyJob> = MemoryStorage::new();
        let notify = Arc::new(tokio::sync::Notify::new());

        let config = JobQueueConfig {
            workers: 1,
            // Long poll interval - if it didn't wake up instantly, it would take 5s
            poll_interval: Duration::from_secs(5),
        };

        let queue = JobQueue::new(config, storage.clone()).unwrap();

        // Wait for worker to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;

        storage
            .push(NotifyJob {
                notify: notify.clone(),
            })
            .await
            .unwrap();

        // Wait for job to be executed
        // If it polls (5s), this will timeout
        let result = tokio::time::timeout(Duration::from_millis(200), notify.notified()).await;
        assert!(result.is_ok(), "Job should be processed instantly");

        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn job_queue_wait_for_job_respects_polling_wait() {
        // CountingStorage uses the DEFAULT wait_for_job (which sleeps)
        let storage = CountingStorage::new();

        let config = JobQueueConfig {
            workers: 1,
            poll_interval: Duration::from_millis(500),
        };

        let queue = JobQueue::new(config, storage.clone()).unwrap();

        // Wait for worker to start and enter sleep (poll interval)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Push job "sideways" directly to inner memory storage
        // Since CountingStorage doesn't forward wait_for_job/push notifications properly (it uses default sleep),
        // the worker is currently sleeping for 500ms.
        storage.push_id(1).await;

        // Check immediate status - should NOT be processed yet (worker is sleeping)
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            storage.count(),
            0,
            "Job processed too early! Worker didn't respect poll interval sleep"
        );

        // Wait enough time for poll interval to expire
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            storage.count(),
            1,
            "Job should have been processed after poll interval"
        );

        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn job_queue_wait_for_job_shutdown_responsiveness() {
        // Use CountingStorage which sleeps for poll_interval
        let storage = CountingStorage::new();

        // Use a LONG poll interval to easily detect if shutdown waits for it
        let poll_interval = Duration::from_secs(5);
        let config = JobQueueConfig {
            workers: 1,
            poll_interval,
        };

        let queue = JobQueue::new(config, storage.clone()).unwrap();

        // Use a background task to call shutdown, so we can measure how long it takes
        let queue_clone = queue.clone();
        let start = std::time::Instant::now();

        // Give worker time to start and enter sleep
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Call shutdown.
        // IDEALLY: It should return instantly (interrupting the sleep).
        // CURRENTLY EXPECTED: It might wait for 5s (poll_interval).
        // This test documents the behavior.
        queue_clone.shutdown().await.unwrap();

        let elapsed = start.elapsed();

        // If it takes > 4s, it means it waited for the full poll interval
        // Ideally we want elapsed < 1s
        assert!(
            elapsed < Duration::from_secs(1),
            "Shutdown took too long: {:?}",
            elapsed
        );
    }
}
