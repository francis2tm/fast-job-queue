//! In-memory storage implementation for testing and simple use cases.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

use crate::{Job, Storage};

/// In-memory job storage.
///
/// Jobs are stored in a thread-safe queue and can be pushed/popped concurrently.
/// This is primarily useful for testing or simple single-process scenarios.
///
/// # Thread Safety
///
/// `MemoryStorage` uses `Arc<Mutex<VecDeque<T>>>` internally, making it safe
/// to share across tasks and threads.
///
/// # Cloning
///
/// `Clone` is implemented manually to avoid requiring `T: Clone`.
/// Cloning creates a new handle to the **same** underlying queue.
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::{MemoryStorage, Storage};
///
/// let storage: MemoryStorage<MyJob> = MemoryStorage::new();
/// storage.push(my_job).await.unwrap();
/// if let Some(job) = storage.pop().await.unwrap() {
///     job.execute(&storage).await.unwrap();
/// }
/// ```
pub struct MemoryStorage<J> {
    jobs: Arc<Mutex<VecDeque<J>>>,
    notify: Arc<Notify>,
}

impl<J> Clone for MemoryStorage<J> {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
            notify: Arc::clone(&self.notify),
        }
    }
}

impl<J> Default for MemoryStorage<J> {
    fn default() -> Self {
        Self::new()
    }
}

impl<J> MemoryStorage<J> {
    /// Create a new empty memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(VecDeque::new())),
            notify: Arc::new(Notify::new()),
        }
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

impl<J: Job<Self> + Send + Sync + 'static> Storage for MemoryStorage<J> {
    type Job = J;
    type Error = Infallible;

    async fn push(&self, job: Self::Job) -> Result<(), Self::Error> {
        self.jobs.lock().await.push_back(job);
        self.notify.notify_one();
        Ok(())
    }

    async fn pop(&self) -> Result<Option<Self::Job>, Self::Error> {
        Ok(self.jobs.lock().await.pop_front())
    }

    async fn wait_for_job(&self, timeout: std::time::Duration) -> Result<(), Self::Error> {
        // Wait for notification OR timeout
        // If we timeout, it just means we loop around and check again (which is fine)
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
    use crate::Storage;

    // Simple job for testing
    #[derive(Debug, PartialEq)]
    struct TestJob {
        id: u64,
    }

    impl Job<MemoryStorage<TestJob>> for TestJob {
        type Error = Infallible;

        async fn execute(self, _storage: &MemoryStorage<TestJob>) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // storage_* tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn storage_new_is_empty() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        assert!(storage.is_empty().await);
        assert_eq!(storage.len().await, 0);
    }

    #[tokio::test]
    async fn storage_default_is_empty() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::default();
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn storage_push_increments_len() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        storage.push(TestJob { id: 1 }).await.unwrap();
        assert_eq!(storage.len().await, 1);
        assert!(!storage.is_empty().await);

        storage.push(TestJob { id: 2 }).await.unwrap();
        assert_eq!(storage.len().await, 2);
    }

    #[tokio::test]
    async fn storage_pop_returns_none_when_empty() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        assert_eq!(storage.pop().await.unwrap(), None);
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn storage_fifo_order() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        storage.push(TestJob { id: 1 }).await.unwrap();
        storage.push(TestJob { id: 2 }).await.unwrap();
        storage.push(TestJob { id: 3 }).await.unwrap();

        assert_eq!(storage.pop().await.unwrap(), Some(TestJob { id: 1 }));
        assert_eq!(storage.pop().await.unwrap(), Some(TestJob { id: 2 }));
        assert_eq!(storage.pop().await.unwrap(), Some(TestJob { id: 3 }));
        assert_eq!(storage.pop().await.unwrap(), None);
    }

    #[tokio::test]
    async fn storage_clone_shares_state() {
        let storage1: MemoryStorage<TestJob> = MemoryStorage::new();
        let storage2 = storage1.clone();

        storage1.push(TestJob { id: 42 }).await.unwrap();
        assert_eq!(storage2.len().await, 1);
        assert_eq!(storage2.pop().await.unwrap(), Some(TestJob { id: 42 }));
        assert!(storage1.is_empty().await);
    }

    #[tokio::test]
    async fn storage_concurrent_pushes() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        let mut handles = vec![];

        for i in 0..100 {
            let storage_clone = storage.clone();
            handles.push(tokio::spawn(async move {
                storage_clone.push(TestJob { id: i }).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(storage.len().await, 100);

        // Verify all items can be popped
        let mut count = 0;
        while storage.pop().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[tokio::test]
    async fn storage_concurrent_push_pop() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();

        let producer = {
            let storage = storage.clone();
            tokio::spawn(async move {
                for i in 0..100 {
                    storage.push(TestJob { id: i }).await.unwrap();
                }
            })
        };

        let consumer = {
            let storage = storage.clone();
            tokio::spawn(async move {
                let mut popped = 0;
                while popped < 100 {
                    if storage.pop().await.unwrap().is_some() {
                        popped += 1;
                    }
                    tokio::task::yield_now().await;
                }
                popped
            })
        };

        producer.await.unwrap();
        assert_eq!(consumer.await.unwrap(), 100);
        assert!(storage.is_empty().await);
    }

    // -------------------------------------------------------------------------
    // wait_for_job_* tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn wait_for_job_wakes_immediately() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        let storage_clone = storage.clone();

        let start = std::time::Instant::now();
        let handle = tokio::spawn(async move {
            // Wait with a long timeout
            storage_clone
                .wait_for_job(std::time::Duration::from_secs(5))
                .await
                .unwrap();
        });

        // Small delay to ensure the other task is waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Push should wake it up immediately
        storage.push(TestJob { id: 1 }).await.unwrap();

        handle.await.unwrap();
        let elapsed = start.elapsed();

        // Should happen almost instantly, definitely much faster than the 5s timeout
        assert!(elapsed < std::time::Duration::from_millis(100));
    }

    #[tokio::test]
    async fn wait_for_job_respects_timeout() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(100);

        // This should timeout because nothing is pushed
        storage.wait_for_job(timeout).await.unwrap();

        let elapsed = start.elapsed();
        // Should wait at least the timeout duration
        assert!(elapsed >= timeout);
    }

    #[tokio::test]
    async fn wait_for_job_only_wakes_one() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        let counter = Arc::new(tokio::sync::Mutex::new(0));

        // Spawn 3 waiting tasks
        for _ in 0..3 {
            let storage = storage.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                storage
                    .wait_for_job(std::time::Duration::from_secs(5))
                    .await
                    .unwrap();
                let mut c = counter.lock().await;
                *c += 1;
            });
        }

        // Give them time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Push ONE job, should wake ONE task
        storage.push(TestJob { id: 1 }).await.unwrap();

        // Give it time to wake up
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let count = *counter.lock().await;
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn wait_for_job_consecutive_timeouts() {
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();
        let timeout = std::time::Duration::from_millis(50);

        let start = std::time::Instant::now();

        // Both should timeout without issue
        storage.wait_for_job(timeout).await.unwrap();
        storage.wait_for_job(timeout).await.unwrap();

        // Should have waited at least 2x timeout
        assert!(start.elapsed() >= timeout * 2);
    }

    // -------------------------------------------------------------------------
    // job_execute_* tests
    // -------------------------------------------------------------------------

    // Job that can spawn a sub-job during execution
    #[derive(Debug, PartialEq)]
    struct SpawningJob {
        id: u64,
        spawn_child: bool,
    }

    impl Job<MemoryStorage<SpawningJob>> for SpawningJob {
        type Error = Infallible;

        async fn execute(self, storage: &MemoryStorage<SpawningJob>) -> Result<(), Self::Error> {
            if self.spawn_child {
                storage
                    .push(SpawningJob {
                        id: self.id + 100,
                        spawn_child: false,
                    })
                    .await?;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn job_execute_can_spawn_new_jobs() {
        let storage: MemoryStorage<SpawningJob> = MemoryStorage::new();

        // Parent job that will spawn a child
        let parent = SpawningJob {
            id: 1,
            spawn_child: true,
        };
        storage.push(parent).await.unwrap();

        // Pop and execute the parent
        let job = storage.pop().await.unwrap().unwrap();
        assert_eq!(job.id, 1);
        job.execute(&storage).await.unwrap();

        // Child job should now be in the queue
        assert_eq!(storage.len().await, 1);
        let child = storage.pop().await.unwrap().unwrap();
        assert_eq!(child.id, 101);
        assert!(!child.spawn_child);
    }
}
