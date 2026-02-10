//! In-memory storage implementation for testing and simple use cases.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

use crate::{Job, Storage, StorageError};

/// In-memory typed job storage.
///
/// This storage is generic and can hold exactly one job type.
///
/// # Example
///
/// ```rust,no_run
/// use fast_job_queue::{Job, MemoryStorage, Storage, StorageError};
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
/// # async fn example() {
/// let storage = MemoryStorage::new();
/// storage.job_push(PrintJob).await;
/// let job = <MemoryStorage<PrintJob> as Storage<PrintJob>>::job_pop(&storage)
///     .await
///     .expect("memory storage cannot fail");
/// assert!(job.is_some());
/// # }
/// ```
pub struct MemoryStorage<JobType> {
    jobs: Arc<Mutex<VecDeque<JobType>>>,
    notify: Arc<Notify>,
}

impl<JobType> Clone for MemoryStorage<JobType> {
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
            notify: self.notify.clone(),
        }
    }
}

impl<JobType> Default for MemoryStorage<JobType> {
    fn default() -> Self {
        Self::new()
    }
}

impl<JobType> MemoryStorage<JobType> {
    /// Create a new empty memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(VecDeque::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Push a typed job into the queue.
    pub async fn job_push(&self, job: JobType) {
        self.jobs.lock().await.push_back(job);
        self.notify.notify_one();
    }

    /// Get the number of jobs in the queue.
    #[must_use = "this returns the count, it doesn't modify the queue"]
    pub async fn len(&self) -> usize {
        self.jobs.lock().await.len()
    }

    /// Check if the queue is empty.
    #[must_use = "this returns a boolean, it doesn't modify the queue"]
    pub async fn is_empty(&self) -> bool {
        self.jobs.lock().await.is_empty()
    }
}

impl<JobType> Storage<JobType> for MemoryStorage<JobType>
where
    JobType: Job,
{
    async fn job_pop(&self) -> Result<Option<JobType>, StorageError> {
        Ok(self.jobs.lock().await.pop_front())
    }

    async fn wait_for_job(&self, timeout: std::time::Duration) -> Result<(), StorageError> {
        let _ = tokio::time::timeout(timeout, self.notify.notified()).await;
        Ok(())
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CounterJob {
        counter: Arc<AtomicUsize>,
    }

    impl Job for CounterJob {
        async fn execute(self) -> Result<(), StorageError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct OrderedJob {
        seen: Arc<Mutex<Vec<u64>>>,
        id: u64,
    }

    impl Job for OrderedJob {
        async fn execute(self) -> Result<(), StorageError> {
            self.seen.lock().await.push(self.id);
            Ok(())
        }
    }

    struct NoopJob;

    impl Job for NoopJob {
        async fn execute(self) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestError;

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test error")
        }
    }

    impl std::error::Error for TestError {}

    struct ErrorJob;

    impl Job for ErrorJob {
        async fn execute(self) -> Result<(), StorageError> {
            Err(Box::new(TestError))
        }
    }

    #[tokio::test]
    async fn storage_new_is_empty() {
        let storage = MemoryStorage::<NoopJob>::new();
        assert!(storage.is_empty().await);
        assert_eq!(storage.len().await, 0);
    }

    #[tokio::test]
    async fn storage_default_is_empty() {
        let storage = MemoryStorage::<NoopJob>::default();
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn storage_pop_returns_none_when_empty() {
        let storage = MemoryStorage::<NoopJob>::new();
        let job = <MemoryStorage<NoopJob> as Storage<NoopJob>>::job_pop(&storage)
            .await
            .unwrap();
        assert!(job.is_none());
    }

    #[tokio::test]
    async fn storage_fifo_order() {
        let storage = MemoryStorage::<OrderedJob>::new();
        let seen = Arc::new(Mutex::new(Vec::<u64>::new()));

        for id in [1_u64, 2, 3] {
            storage
                .job_push(OrderedJob {
                    seen: seen.clone(),
                    id,
                })
                .await;
        }

        let first = <MemoryStorage<OrderedJob> as Storage<OrderedJob>>::job_pop(&storage)
            .await
            .unwrap()
            .unwrap();
        first.execute().await.unwrap();
        let second = <MemoryStorage<OrderedJob> as Storage<OrderedJob>>::job_pop(&storage)
            .await
            .unwrap()
            .unwrap();
        second.execute().await.unwrap();
        let third = <MemoryStorage<OrderedJob> as Storage<OrderedJob>>::job_pop(&storage)
            .await
            .unwrap()
            .unwrap();
        third.execute().await.unwrap();
        let none = <MemoryStorage<OrderedJob> as Storage<OrderedJob>>::job_pop(&storage)
            .await
            .unwrap();
        assert!(none.is_none());

        assert_eq!(*seen.lock().await, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn storage_clone_shares_state() {
        let storage1 = MemoryStorage::<CounterJob>::new();
        let storage2 = storage1.clone();
        let counter = Arc::new(AtomicUsize::new(0));

        storage1
            .job_push(CounterJob {
                counter: counter.clone(),
            })
            .await;

        assert_eq!(storage2.len().await, 1);

        let job = <MemoryStorage<CounterJob> as Storage<CounterJob>>::job_pop(&storage2)
            .await
            .unwrap()
            .unwrap();
        job.execute().await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(storage1.is_empty().await);
    }

    #[tokio::test]
    async fn storage_concurrent_pushes() {
        let storage = MemoryStorage::<CounterJob>::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        for _ in 0..100 {
            let storage = storage.clone();
            let counter = counter.clone();
            handles.push(tokio::spawn(async move {
                storage.job_push(CounterJob { counter }).await;
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(storage.len().await, 100);
        for _ in 0..100 {
            let job = <MemoryStorage<CounterJob> as Storage<CounterJob>>::job_pop(&storage)
                .await
                .unwrap()
                .unwrap();
            job.execute().await.unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 100);
    }

    #[tokio::test]
    async fn wait_for_job_wakes_immediately() {
        let storage = MemoryStorage::<NoopJob>::new();
        let storage_clone = storage.clone();

        let start = std::time::Instant::now();
        let handle = tokio::spawn(async move {
            <MemoryStorage<NoopJob> as Storage<NoopJob>>::wait_for_job(
                &storage_clone,
                std::time::Duration::from_secs(5),
            )
            .await
            .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        storage.job_push(NoopJob).await;

        handle.await.unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[tokio::test]
    async fn wait_for_job_respects_timeout() {
        let storage = MemoryStorage::<NoopJob>::new();
        let timeout = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();

        <MemoryStorage<NoopJob> as Storage<NoopJob>>::wait_for_job(&storage, timeout)
            .await
            .unwrap();
        assert!(start.elapsed() >= timeout);
    }

    #[tokio::test]
    async fn execute_propagates_job_error() {
        let storage = MemoryStorage::<ErrorJob>::new();

        storage.job_push(ErrorJob).await;

        let job = <MemoryStorage<ErrorJob> as Storage<ErrorJob>>::job_pop(&storage)
            .await
            .unwrap()
            .unwrap();
        let result = job.execute().await;
        assert!(result.is_err());
    }
}
