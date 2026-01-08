//! In-memory storage implementation for testing and simple use cases.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Mutex;

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
/// ```rust
/// use fast_job_queue::MemoryStorage;
///
/// # tokio_test::block_on(async {
/// let storage: MemoryStorage<String> = MemoryStorage::new();
///
/// storage.push("job1".to_string()).await;
/// storage.push("job2".to_string()).await;
///
/// assert_eq!(storage.len().await, 2);
/// assert_eq!(storage.pop().await, Some("job1".to_string()));
/// # });
/// ```
pub struct MemoryStorage<T> {
    jobs: Arc<Mutex<VecDeque<T>>>,
}

impl<T> Clone for MemoryStorage<T> {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
        }
    }
}

impl<T> Default for MemoryStorage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> MemoryStorage<T> {
    /// Create a new empty memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Push a job to the back of the queue.
    pub async fn push(&self, job: T) {
        self.jobs.lock().await.push_back(job);
    }

    /// Pop a job from the front of the queue.
    ///
    /// Returns `None` if the queue is empty.
    pub async fn pop(&self) -> Option<T> {
        self.jobs.lock().await.pop_front()
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

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_storage_new_is_empty() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();
        assert!(storage.is_empty().await);
        assert_eq!(storage.len().await, 0);
    }

    #[tokio::test]
    async fn memory_storage_default_is_empty() {
        let storage: MemoryStorage<String> = MemoryStorage::default();
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn memory_storage_push_increments_len() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();
        storage.push(1).await;
        assert_eq!(storage.len().await, 1);
        assert!(!storage.is_empty().await);

        storage.push(2).await;
        assert_eq!(storage.len().await, 2);
    }

    #[tokio::test]
    async fn memory_storage_pop_returns_none_when_empty() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();
        assert_eq!(storage.pop().await, None);
        // Popping from empty should remain stable
        assert_eq!(storage.pop().await, None);
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn memory_storage_fifo_order() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();
        storage.push(1).await;
        storage.push(2).await;
        storage.push(3).await;

        assert_eq!(storage.pop().await, Some(1));
        assert_eq!(storage.pop().await, Some(2));
        assert_eq!(storage.pop().await, Some(3));
        assert_eq!(storage.pop().await, None);
    }

    #[tokio::test]
    async fn memory_storage_interleaved_push_pop() {
        let storage: MemoryStorage<&str> = MemoryStorage::new();
        storage.push("a").await;
        assert_eq!(storage.pop().await, Some("a"));

        storage.push("b").await;
        storage.push("c").await;
        assert_eq!(storage.pop().await, Some("b"));

        storage.push("d").await;
        assert_eq!(storage.pop().await, Some("c"));
        assert_eq!(storage.pop().await, Some("d"));
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn memory_storage_clone_shares_state() {
        let storage1: MemoryStorage<i32> = MemoryStorage::new();
        let storage2 = storage1.clone();

        storage1.push(42).await;
        assert_eq!(storage2.len().await, 1);
        assert_eq!(storage2.pop().await, Some(42));
        assert!(storage1.is_empty().await);
    }

    #[tokio::test]
    async fn memory_storage_concurrent_pushes() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();
        let mut handles = vec![];

        for i in 0..100 {
            let storage_clone = storage.clone();
            handles.push(tokio::spawn(async move {
                storage_clone.push(i).await;
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(storage.len().await, 100);

        // Verify all items can be popped
        let mut count = 0;
        while storage.pop().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[tokio::test]
    async fn memory_storage_concurrent_push_and_pop() {
        let storage: MemoryStorage<i32> = MemoryStorage::new();

        // Pre-populate
        for i in 0..50 {
            storage.push(i).await;
        }

        let storage_push = storage.clone();
        let storage_pop = storage.clone();

        let push_handle = tokio::spawn(async move {
            for i in 50..100 {
                storage_push.push(i).await;
            }
        });

        let pop_handle = tokio::spawn(async move {
            let mut popped = 0;
            for _ in 0..50 {
                if storage_pop.pop().await.is_some() {
                    popped += 1;
                }
            }
            popped
        });

        push_handle.await.unwrap();
        let popped = pop_handle.await.unwrap();

        // We pushed 100 total, popped up to 50
        assert_eq!(popped, 50);
        assert_eq!(storage.len().await, 50);
    }

    #[tokio::test]
    async fn memory_storage_with_complex_type() {
        #[derive(Debug, PartialEq)]
        struct Job {
            id: u64,
            payload: String,
        }

        let storage: MemoryStorage<Job> = MemoryStorage::new();
        storage
            .push(Job {
                id: 1,
                payload: "first".into(),
            })
            .await;
        storage
            .push(Job {
                id: 2,
                payload: "second".into(),
            })
            .await;

        let job = storage.pop().await.unwrap();
        assert_eq!(job.id, 1);
        assert_eq!(job.payload, "first");
    }
}
