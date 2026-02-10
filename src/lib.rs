//! A storage-agnostic async job queue with configurable workers.
//!
//! This crate provides a generic job queue that can work with any storage backend.
//! Built-in implementations are provided:
//!
//! - [`MemoryStorage`] - In-memory queue for testing
//! - Custom storage - Implement [`Storage<JobType>`] for your backend
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────┐     ┌─────────────┐     ┌─────────────┐
//! │ Storage<A::Job>    │◄────│             │────►│  Job::execute│
//! ├────────────────────┤     │  JobQueue   │     ├─────────────┤
//! │ Storage<B::Job>    │◄────│             │────►│  Job::execute│
//! └────────────────────┘     └─────────────┘     └─────────────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use fast_job_queue::{Job, JobQueue, MemoryStorage, StorageError};
//!
//! struct PrintJob;
//!
//! impl Job for PrintJob {
//!     async fn execute(self) -> Result<(), StorageError> {
//!         println!("Processing task");
//!         Ok(())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), fast_job_queue::JobQueueError> {
//!     let storage_a: MemoryStorage<PrintJob> = MemoryStorage::new();
//!     let storage_b: MemoryStorage<PrintJob> = MemoryStorage::new();
//!
//!     let queue = JobQueue::builder()
//!         .workers(4)
//!         .poll_interval(Duration::from_millis(100))
//!         .with_storage(storage_a.clone())
//!         .with_storage(storage_b.clone())
//!         .build()?;
//!
//!     storage_a.job_push(PrintJob).await;
//!
//!     tokio::time::sleep(Duration::from_millis(100)).await;
//!
//!     queue.shutdown().await?;
//!     Ok(())
//! }
//! ```

mod error;
mod queue;
mod storage;

pub use error::JobQueueError;
pub use queue::{JobQueue, JobQueueBuilder};
pub use storage::{ExecuteOutcome, Job, MemoryStorage, Storage, StorageError};
