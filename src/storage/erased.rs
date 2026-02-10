//! Runtime list for heterogeneous typed storages.

use super::{ExecuteOutcome, Job, Storage, StorageError};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

/// Type-erased storage entry used by queue workers.
#[async_trait::async_trait]
pub trait StorageListItem: Send + Sync + 'static {
    /// Execute one job from this storage if available.
    async fn execute_one(&self) -> Result<ExecuteOutcome, StorageError>;

    /// Wait for new jobs until timeout.
    async fn wait_for_job(&self, timeout: Duration) -> Result<(), StorageError>;
}

struct StorageAdapter<StorageType, JobType> {
    storage: StorageType,
    _job: PhantomData<fn() -> JobType>,
}

impl<StorageType, JobType> StorageAdapter<StorageType, JobType> {
    fn new(storage: StorageType) -> Self {
        Self {
            storage,
            _job: PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<StorageType, JobType> StorageListItem for StorageAdapter<StorageType, JobType>
where
    StorageType: Storage<JobType>,
    JobType: Job,
{
    async fn execute_one(&self) -> Result<ExecuteOutcome, StorageError> {
        match self.storage.job_pop().await? {
            Some(job) => {
                job.execute().await?;
                Ok(ExecuteOutcome::Executed)
            }
            None => Ok(ExecuteOutcome::Empty),
        }
    }

    async fn wait_for_job(&self, timeout: Duration) -> Result<(), StorageError> {
        self.storage.wait_for_job(timeout).await
    }
}

/// Collection of heterogeneous typed storages.
#[derive(Default)]
pub struct StorageList {
    items: Vec<Arc<dyn StorageListItem>>,
}

impl StorageList {
    /// Create an empty storage list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one typed storage to the list.
    pub fn storage_push<StorageType, JobType>(&mut self, storage: StorageType)
    where
        StorageType: Storage<JobType>,
        JobType: Job,
    {
        self.items
            .push(Arc::new(StorageAdapter::<StorageType, JobType>::new(
                storage,
            )));
    }

    /// Returns true when there are no storage entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Convert into an `Arc` slice for cheap worker sharing.
    #[must_use]
    pub fn arc_slice_into(self) -> Arc<[Arc<dyn StorageListItem>]> {
        Arc::from(self.items.into_boxed_slice())
    }
}
