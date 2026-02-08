//! Job queue implementation with configurable workers.

use crate::storage::{StorageList, StorageListItem};
use crate::{ExecuteOutcome, Job, JobQueueError, Storage};
use futures::future::select_all;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

const STORAGE_ERROR_BACKOFF: Duration = Duration::from_millis(50);

async fn storage_execute_round_robin(
    storages: &[Arc<dyn StorageListItem>],
    next_storage: &mut usize,
) -> bool {
    for _ in 0..storages.len() {
        let index = *next_storage;
        *next_storage = (*next_storage + 1) % storages.len();

        match storages[index].execute_one().await {
            Ok(ExecuteOutcome::Executed) => return true,
            Ok(ExecuteOutcome::Empty) => {}
            Err(error) => warn!(storage_index = index, error = %error, "Storage execute failed"),
        }
    }

    false
}

async fn storages_wait_for_any(storages: &[Arc<dyn StorageListItem>], timeout: Duration) {
    let waiters: Vec<_> = storages
        .iter()
        .map(|storage| storage.wait_for_job(timeout))
        .collect();

    let (result, _, _) = select_all(waiters).await;
    if let Err(error) = result {
        warn!(error = %error, "Storage wait failed");
        tokio::time::sleep(STORAGE_ERROR_BACKOFF).await;
    }
}

async fn worker_run(
    worker_id: usize,
    storages: Arc<[Arc<dyn StorageListItem>]>,
    poll_interval: Duration,
    cancellation: CancellationToken,
) {
    let mut next_storage = worker_id % storages.len();
    debug!(worker_id, "Worker starting");

    loop {
        if cancellation.is_cancelled() {
            debug!(worker_id, "Worker received shutdown signal");
            break;
        }

        if storage_execute_round_robin(&storages, &mut next_storage).await {
            debug!(worker_id, "Worker executed job");
            continue;
        }

        tokio::select! {
            _ = cancellation.cancelled() => {
                debug!(worker_id, "Worker received shutdown signal while waiting");
                break;
            }
            _ = storages_wait_for_any(&storages, poll_interval) => {}
        }
    }

    debug!(worker_id, "Worker shutting down");
}

/// Builder for job queues.
///
/// Use [`JobQueue::builder()`] to create a new builder.
pub struct JobQueueBuilder {
    workers: usize,
    poll_interval: Duration,
    storages: StorageList,
}

impl Default for JobQueueBuilder {
    fn default() -> Self {
        Self {
            workers: 4,
            poll_interval: Duration::from_secs(1),
            storages: StorageList::new(),
        }
    }
}

impl JobQueueBuilder {
    /// Create a new job queue builder with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

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
    pub fn with_storage<StorageType, JobType>(mut self, storage: StorageType) -> Self
    where
        StorageType: Storage<JobType>,
        JobType: Job,
    {
        self.storages.storage_push(storage);
        self
    }

    /// Add a pre-shared storage backend to the queue.
    pub fn with_storage_arc<StorageType, JobType>(mut self, storage: Arc<StorageType>) -> Self
    where
        StorageType: Storage<JobType>,
        JobType: Job,
    {
        self.storages.storage_push(storage);
        self
    }

    /// Build and start the queue, spawning worker tasks.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`JobQueueError::WorkersMustBePositive`] if `workers` is 0
    /// - [`JobQueueError::PollIntervalMustBePositive`] if `poll_interval` is 0
    /// - [`JobQueueError::MissingStorages`] if no storage backends are configured
    #[must_use = "job queue must be stored to keep workers running"]
    pub fn build(self) -> Result<JobQueue, JobQueueError> {
        if self.workers == 0 {
            return Err(JobQueueError::WorkersMustBePositive);
        }

        if self.poll_interval.is_zero() {
            return Err(JobQueueError::PollIntervalMustBePositive);
        }

        if self.storages.is_empty() {
            return Err(JobQueueError::MissingStorages);
        }

        let storages = self.storages.arc_slice_into();
        let cancellation = CancellationToken::new();
        let mut workers = JoinSet::new();

        for worker_id in 0..self.workers {
            let storages = storages.clone();
            let poll_interval = self.poll_interval;
            let cancellation = cancellation.clone();
            workers.spawn(async move {
                worker_run(worker_id, storages, poll_interval, cancellation).await;
            });
        }

        Ok(JobQueue {
            workers,
            cancellation,
        })
    }
}

/// A running job queue.
///
/// Use [`JobQueue::builder()`] to create and configure the queue.
pub struct JobQueue {
    workers: JoinSet<()>,
    cancellation: CancellationToken,
}

impl JobQueue {
    /// Create a new job queue builder.
    pub fn builder() -> JobQueueBuilder {
        JobQueueBuilder::new()
    }

    /// Gracefully shutdown the queue.
    ///
    /// Signals all workers to stop and waits for them to finish their current job.
    pub async fn shutdown(mut self) -> Result<(), JobQueueError> {
        self.cancellation.cancel();

        while let Some(join_result) = self.workers.join_next().await {
            join_result.map_err(|error| JobQueueError::WorkerPanicked {
                reason: error.to_string(),
            })?;
        }

        info!("All workers shut down successfully");
        Ok(())
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.workers.detach_all();
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Job, JobQueueError, MemoryStorage, StorageError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    enum TestJob {
        Increment(Arc<AtomicUsize>),
        Notify(Arc<tokio::sync::Notify>),
        Sleep(Duration),
    }

    impl Job for TestJob {
        async fn execute(self) -> Result<(), StorageError> {
            match self {
                Self::Increment(counter) => {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                Self::Notify(notify) => {
                    notify.notify_one();
                }
                Self::Sleep(duration) => {
                    tokio::time::sleep(duration).await;
                }
            }

            Ok(())
        }
    }

    type TestStorage = MemoryStorage<TestJob>;

    #[test]
    fn builder_default_has_sensible_values() {
        let builder = JobQueueBuilder::default();
        assert_eq!(builder.workers, 4);
        assert_eq!(builder.poll_interval, Duration::from_secs(1));
        assert!(builder.storages.is_empty());
    }

    #[tokio::test]
    async fn build_rejects_zero_workers() {
        let storage = TestStorage::new();

        let result = JobQueue::builder()
            .workers(0)
            .poll_interval(Duration::from_millis(100))
            .with_storage(storage)
            .build();

        assert!(matches!(result, Err(JobQueueError::WorkersMustBePositive)));
    }

    #[tokio::test]
    async fn build_rejects_zero_poll_interval() {
        let storage = TestStorage::new();

        let result = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(0))
            .with_storage(storage)
            .build();

        assert!(matches!(
            result,
            Err(JobQueueError::PollIntervalMustBePositive)
        ));
    }

    #[test]
    fn build_rejects_no_storages() {
        let result = JobQueue::builder().workers(1).build();
        assert!(matches!(result, Err(JobQueueError::MissingStorages)));
    }

    async fn counter_wait(counter: &Arc<AtomicUsize>, target: usize) {
        let wait = async {
            while counter.load(Ordering::SeqCst) < target {
                tokio::task::yield_now().await;
            }
        };

        tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .expect("timed out waiting for expected counter value");
    }

    #[tokio::test]
    async fn processes_single_job() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage = TestStorage::new();
        storage.job_push(TestJob::Increment(counter.clone())).await;

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        counter_wait(&counter, 1).await;
        queue.shutdown().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn processes_multiple_jobs() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage = TestStorage::new();

        for _ in 0..5 {
            storage.job_push(TestJob::Increment(counter.clone())).await;
        }

        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage)
            .build()
            .unwrap();

        counter_wait(&counter, 5).await;
        queue.shutdown().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn shutdown_is_graceful() {
        let storage = TestStorage::new();

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
        let storage = TestStorage::new();

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_millis(50))
            .with_storage(storage)
            .build()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), queue.shutdown())
            .await
            .expect("shutdown timed out")
            .unwrap();
    }

    #[tokio::test]
    async fn wait_for_job_processes_instantly() {
        let storage = TestStorage::new();
        let notify = Arc::new(tokio::sync::Notify::new());
        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_secs(5))
            .with_storage(storage.clone())
            .build()
            .unwrap();

        storage.job_push(TestJob::Notify(notify.clone())).await;

        let result = tokio::time::timeout(Duration::from_millis(200), notify.notified()).await;
        assert!(result.is_ok(), "Job should be processed quickly");

        queue.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_is_responsive() {
        let storage = TestStorage::new();

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_secs(5))
            .with_storage(storage)
            .build()
            .unwrap();

        let start = std::time::Instant::now();
        queue.shutdown().await.unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "Shutdown took too long: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn multi_storage_processes_from_all() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage_a = TestStorage::new();
        let storage_b = TestStorage::new();

        for storage in [storage_a.clone(), storage_b.clone()] {
            storage.job_push(TestJob::Increment(counter.clone())).await;
        }

        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(10))
            .with_storage(storage_a)
            .with_storage(storage_b)
            .build()
            .unwrap();

        counter_wait(&counter, 2).await;
        queue.shutdown().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn multi_storage_round_robin_prevents_starvation() {
        let fast_notify = Arc::new(tokio::sync::Notify::new());
        let fast_storage = TestStorage::new();
        let busy_storage = TestStorage::new();

        for _ in 0..10 {
            busy_storage
                .job_push(TestJob::Sleep(Duration::from_millis(50)))
                .await;
        }

        fast_storage
            .job_push(TestJob::Notify(fast_notify.clone()))
            .await;

        let queue = JobQueue::builder()
            .workers(1)
            .poll_interval(Duration::from_secs(1))
            .with_storage(fast_storage)
            .with_storage(busy_storage)
            .build()
            .unwrap();

        let result = tokio::time::timeout(Duration::from_millis(200), fast_notify.notified()).await;
        queue.shutdown().await.unwrap();
        assert!(
            result.is_ok(),
            "Expected non-starved storage to be processed quickly"
        );
    }
}
