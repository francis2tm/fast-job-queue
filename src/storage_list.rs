//! Composite storage list for multi-storage queues.
//!
//! This module provides the [`StorageList`] trait for combining multiple
//! storage backends into a single queue.

use crate::{Job, Storage};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::error;

/// A boxed job future returned by `pop_any`.
pub type JobFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Trait for composite storage (tuple of storages).
///
/// Implemented for nested tuples like `(Arc<A>, (Arc<B>, ()))`.
pub trait StorageList: Clone + Send + Sync + 'static {
    /// Pop a job from any storage in the list.
    ///
    /// Returns the first available job as a boxed future, or `None` if all storages are empty.
    fn pop_any(&self) -> impl Future<Output = Option<JobFuture>> + Send;

    /// Wait for any storage to have a job available.
    ///
    /// This uses `tokio::select!` to wait on all storages concurrently.
    fn wait_for_any(&self, timeout: Duration) -> impl Future<Output = ()> + Send;
}

// =============================================================================
// Base Case: Empty Tuple
// =============================================================================

impl StorageList for () {
    async fn pop_any(&self) -> Option<JobFuture> {
        None
    }

    async fn wait_for_any(&self, timeout: Duration) {
        tokio::time::sleep(timeout).await;
    }
}

impl<S, T> StorageList for (Arc<S>, T)
where
    S: Storage,
    T: StorageList,
{
    async fn pop_any(&self) -> Option<JobFuture> {
        // Try this storage first
        match self.0.pop().await {
            Ok(Some(job)) => {
                let storage = self.0.clone();
                return Some(Box::pin(async move {
                    if let Err(e) = job.execute(&storage).await {
                        error!(error = %e, "Job execution failed");
                    }
                }));
            }
            Ok(None) => {}
            Err(e) => {
                error!(error = %e, "Failed to pop job from storage");
            }
        }

        // Then try the tail
        self.1.pop_any().await
    }

    async fn wait_for_any(&self, timeout: Duration) {
        // Wait on both this storage and the tail concurrently
        tokio::select! {
            _ = self.0.wait_for_job(timeout) => {}
            _ = self.1.wait_for_any(timeout) => {}
        }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStorage;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct TestJob {
        counter: Arc<AtomicUsize>,
    }

    impl Job<MemoryStorage<TestJob>> for TestJob {
        type Error = Infallible;

        async fn execute(self, _storage: &MemoryStorage<TestJob>) -> Result<(), Infallible> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn empty_list_returns_none() {
        let list: () = ();
        assert!(list.pop_any().await.is_none());
    }

    #[tokio::test]
    async fn single_storage_pops_job() {
        let counter = Arc::new(AtomicUsize::new(0));
        let storage: MemoryStorage<TestJob> = MemoryStorage::new();

        storage
            .push(TestJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();

        let list = (Arc::new(storage), ());

        let job = list.pop_any().await.expect("Should have job");
        job.await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn multi_storage_pops_from_both() {
        let counter = Arc::new(AtomicUsize::new(0));

        let storage1: MemoryStorage<TestJob> = MemoryStorage::new();
        let storage2: MemoryStorage<TestJob> = MemoryStorage::new();

        storage1
            .push(TestJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();
        storage2
            .push(TestJob {
                counter: counter.clone(),
            })
            .await
            .unwrap();

        let list = (Arc::new(storage1), (Arc::new(storage2), ()));

        // First pop
        let job1 = list.pop_any().await.expect("Should have job");
        job1.await;

        // Second pop (from storage2)
        let job2 = list.pop_any().await.expect("Should have job");
        job2.await;

        // Third pop should be None
        assert!(list.pop_any().await.is_none());

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
