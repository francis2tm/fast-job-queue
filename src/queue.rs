//! Job queue implementation with configurable workers.

use crate::StorageList;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, info};

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

/// Builder for job queues.
///
/// Use [`JobQueue::builder()`] to create a new builder.
pub struct JobQueueBuilder<S> {
    workers: usize,
    poll_interval: Duration,
    storages: S,
}

impl Default for JobQueueBuilder<()> {
    fn default() -> Self {
        Self {
            workers: 4,
            poll_interval: Duration::from_secs(1),
            storages: (),
        }
    }
}

impl JobQueueBuilder<()> {
    /// Create a new job queue builder with default configuration.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: StorageList> JobQueueBuilder<S> {
    /// Set the number of concurrent worker tasks.
    /// Default is 4.
    pub fn workers(mut self, workers: usize) -> Self {
        self.workers = workers;
        self
    }

    /// Set the poll interval when no jobs are available.
    /// Default is 1 second.
    pub fn poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Add a storage backend to the queue.
    ///
    /// Multiple storages can be added; workers poll all of them.
    pub fn with_storage<T: crate::Storage>(self, storage: T) -> JobQueueBuilder<(Arc<T>, S)> {
        JobQueueBuilder {
            workers: self.workers,
            poll_interval: self.poll_interval,
            storages: (Arc::new(storage), self.storages),
        }
    }

    /// Build and start the queue, spawning worker tasks.
    ///
    /// # Errors
    ///
    /// Returns [`JobQueueError::InvalidConfig`] if:
    /// - `workers` is 0
    /// - `poll_interval` is 0
    #[must_use = "job queue must be stored to keep workers running"]
    pub fn build(self) -> Result<JobQueue<S>, JobQueueError> {
        if self.workers == 0 {
            return Err(JobQueueError::InvalidConfig(
                "workers must be greater than 0".into(),
            ));
        }

        if self.poll_interval == Duration::from_millis(0) {
            return Err(JobQueueError::InvalidConfig(
                "poll_interval must be greater than 0".into(),
            ));
        }

        let (shutdown_tx, _) = broadcast::channel(1);
        let storages = Arc::new(self.storages);

        let mut workers = Vec::with_capacity(self.workers);
        for worker_id in 0..self.workers {
            let storages = storages.clone();
            let poll_interval = self.poll_interval;
            let mut shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                debug!(worker_id, "Worker starting");

                loop {
                    // Check for shutdown signal
                    if shutdown_rx.try_recv().is_ok() {
                        debug!(worker_id, "Worker received shutdown signal");
                        break;
                    }

                    // Pop (claim) the next pending job from any storage
                    if let Some(job) = storages.pop_any().await {
                        debug!(worker_id, "Worker claimed job");
                        job.await;
                        debug!(worker_id, "Job completed");
                    } else {
                        // No jobs available, wait for jobs OR shutdown signal
                        tokio::select! {
                            _ = storages.wait_for_any(poll_interval) => {}
                            _ = shutdown_rx.recv() => {
                                debug!(worker_id, "Worker received shutdown signal while waiting");
                                break;
                            }
                        }
                    }
                }

                debug!(worker_id, "Worker shutting down");
            });

            workers.push(handle);
        }

        let s = match Arc::try_unwrap(storages) {
            Ok(s) => s,
            Err(arc_s) => (*arc_s).clone(),
        };

        Ok(JobQueue {
            storages: s,
            inner: Arc::new(JobQueueInner {
                workers: Mutex::new(workers),
                shutdown_tx,
            }),
        })
    }
}

struct JobQueueInner {
    workers: Mutex<Vec<JoinHandle<()>>>,
    shutdown_tx: broadcast::Sender<()>,
}

/// A running job queue.
///
/// Use [`JobQueue::builder()`] to create and configure the queue.
pub struct JobQueue<S> {
    storages: S,
    inner: Arc<JobQueueInner>,
}

impl JobQueue<()> {
    /// Create a new job queue builder.
    pub fn builder() -> JobQueueBuilder<()> {
        JobQueueBuilder::new()
    }
}

impl<S: StorageList> JobQueue<S> {
    /// Gracefully shutdown the queue.
    ///
    /// Signals all workers to stop and waits for them to finish their current job.
    pub async fn shutdown(&self) -> Result<(), JobQueueError> {
        let _ = self.inner.shutdown_tx.send(());
        let mut workers = self.inner.workers.lock().await;

        for (idx, handle) in workers.drain(..).enumerate() {
            handle.await.map_err(|e| {
                JobQueueError::WorkerPanicked(format!("Worker {} panicked: {}", idx, e))
            })?;
        }

        info!("All workers shut down successfully");
        Ok(())
    }
}

impl<S: Clone> Clone for JobQueue<S> {
    fn clone(&self) -> Self {
        Self {
            storages: self.storages.clone(),
            inner: self.inner.clone(),
        }
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
    // Builder Tests
    // =========================================================================

    #[test]
    fn builder_default_has_sensible_values() {
        let builder = JobQueueBuilder::default();
        assert_eq!(builder.workers, 4);
        assert_eq!(builder.poll_interval, Duration::from_secs(1));
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
    async fn build_rejects_zero_workers() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let result = JobQueue::builder()
            .workers(0)
            .poll_interval(Duration::from_millis(100))
            .with_storage(storage)
            .build();

        match result {
            Err(e) => assert!(e.to_string().contains("workers must be greater than 0")),
            Ok(_) => panic!("Expected error for zero workers"),
        }
    }

    #[tokio::test]
    async fn build_rejects_zero_poll_interval() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let result = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(0))
            .with_storage(storage)
            .build();

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
    struct CountingJob {
        counter: Arc<AtomicUsize>,
    }

    impl Job<MemoryStorage<CountingJob>> for CountingJob {
        type Error = Infallible;

        async fn execute(self, _storage: &MemoryStorage<CountingJob>) -> Result<(), Infallible> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn processes_single_job() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage: MemoryStorage<CountingJob> = MemoryStorage::new();
        storage
            .push(CountingJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        queue.shutdown().await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn processes_multiple_jobs() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage: MemoryStorage<CountingJob> = MemoryStorage::new();

        for _ in 0..5 {
            storage
                .push(CountingJob {
                    counter: counter.clone(),
                })
                .await
                .unwrap();
        }

        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        queue.shutdown().await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn shutdown_is_graceful() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        let result = queue.shutdown().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn idle_workers_respect_timeout() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_millis(50))
            .with_storage(storage)
            .build()
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn queue_clone_shares_state() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();

        let queue1 = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        let queue2 = queue1.clone();

        // Shutdown through one handle should stop workers for both
        queue1.shutdown().await.unwrap();

        // Second shutdown should complete immediately
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

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_secs(5))
            .with_storage(storage.clone())
            .build()
            .unwrap();

        // Wait for worker to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;

        storage
            .push(NotifyJob {
                notify: notify.clone(),
            })
            .await
            .unwrap();

        // Should process instantly (not wait 5s)
        let result = tokio::time::timeout(Duration::from_millis(200), notify.notified()).await;
        assert!(result.is_ok(), "Job should be processed instantly");

        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn job_queue_wait_for_job_shutdown_responsiveness() {
        let storage: MemoryStorage<SimpleJob> = MemoryStorage::new();
        let poll_interval = Duration::from_secs(5);

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(poll_interval)
            .with_storage(storage)
            .build()
            .unwrap();

        let queue_clone = queue.clone();
        let start = std::time::Instant::now();

        // Give worker time to enter waiting state
        tokio::time::sleep(Duration::from_millis(50)).await;

        queue_clone.shutdown().await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "Shutdown took too long: {:?}",
            elapsed
        );
    }

    // =========================================================================
    // Multi-Storage Tests
    // =========================================================================

    #[tokio::test]
    async fn multi_storage_processes_from_all() {
        let counter = Arc::new(AtomicUsize::new(0));

        let storage_a: MemoryStorage<CountingJob> = MemoryStorage::new();
        let storage_b: MemoryStorage<CountingJob> = MemoryStorage::new();

        // Push to both storages
        storage_a
            .push(CountingJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();
        storage_b
            .push(CountingJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();

        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage_a)
            .with_storage(storage_b)
            .build()
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        queue.shutdown().await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
