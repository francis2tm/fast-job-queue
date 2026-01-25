//! A storage-agnostic async job queue with configurable workers.
//!
//! This crate provides a generic job queue that can work with any storage backend.
//! Built-in implementations are provided:
//!
//! - [`MemoryStorage`] - In-memory queue for testing
//! - Custom storage - Implement the [`Storage`] trait for your backend
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │   Storage   │◄────│  JobQueue   │────►│    Job      │
//! │(Memory, DB) │     │ <S:Storage> │     │  (Your Type)│
//! └─────────────┘     └─────────────┘     └─────────────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use std::convert::Infallible;
//! use fast_job_queue::{Job, JobQueue, JobQueueConfig, MemoryStorage, Storage};
//!
//! // Define your job type
//! struct MyJob { id: u64, data: String }
//!
//! // Implement Job for your storage
//! impl Job<MemoryStorage<MyJob>> for MyJob {
//!     type Error = Infallible;
//!
//!     async fn execute(self, _storage: &MemoryStorage<MyJob>) -> Result<(), Infallible> {
//!         println!("Processing job {}: {}", self.id, self.data);
//!         Ok(())
//!     }
//! }
//!
//! // Create storage and queue
//! let storage = MemoryStorage::new();
//! let queue = JobQueue::new(JobQueueConfig::default(), storage.clone())?;
//!
//! // Push jobs through storage
//! storage.push(MyJob { id: 1, data: "task".into() }).await?;
//!
//! // Queue processes jobs in the background...
//! tokio::time::sleep(std::time::Duration::from_millis(100)).await;
//!
//! queue.shutdown().await?;
//! ```

mod job;
mod queue;
mod storage;

pub use job::Job;
pub use queue::{JobQueue, JobQueueConfig, JobQueueError};
pub use storage::{MemoryStorage, Storage};
