//! In-memory storage implementation for testing and simple use cases.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::Job;

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
}

impl<J> Clone for MemoryStorage<J> {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
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

impl<J: Job<Self> + Send + Sync + 'static> super::Storage for MemoryStorage<J> {
    type Job = J;
    type Error = Infallible;

    async fn push(&self, job: Self::Job) -> Result<(), Self::Error> {
        self.jobs.lock().await.push_back(job);
        Ok(())
    }

    async fn pop(&self) -> Result<Option<Self::Job>, Self::Error> {
        Ok(self.jobs.lock().await.pop_front())
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
}
