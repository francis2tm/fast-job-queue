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
//! │ Storage A   │◄────│             │────►│    Job A    │
//! ├─────────────┤     │  JobQueue   │     ├─────────────┤
//! │ Storage B   │◄────│             │────►│    Job B    │
//! └─────────────┘     └─────────────┘     └─────────────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use std::convert::Infallible;
//! use std::time::Duration;
//! use fast_job_queue::{Job, JobQueue, MemoryStorage, Storage};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), fast_job_queue::JobQueueError> {
//!     // Define your job type
//!     struct MyJob { id: u64, data: String }
//!
//!     // Implement Job for your storage
//!     impl Job<MemoryStorage<MyJob>> for MyJob {
//!         type Error = Infallible;
//!
//!         async fn execute(self, _storage: &MemoryStorage<MyJob>) -> Result<(), Infallible> {
//!             println!("Processing job {}: {}", self.id, self.data);
//!             Ok(())
//!         }
//!     }
//!
//!     // Create storages and queue
//!     let storage_a = MemoryStorage::<MyJob>::new();
//!     let storage_b = MemoryStorage::<MyJob>::new();
//!
//!     let queue = JobQueue::builder()
//!         .workers(4)
//!         .poll_interval(Duration::from_millis(100))
//!         .with_storage(storage_a.clone())
//!         .with_storage(storage_b.clone())
//!         .build()?;
//!
//!     // Push jobs through storage
//!     storage_a
//!         .push(MyJob { id: 1, data: "task".into() })
//!         .await
//!         .expect("memory storage push cannot fail");
//!
//!     // Queue processes jobs in the background...
//!     tokio::time::sleep(Duration::from_millis(100)).await;
//!
//!     queue.shutdown().await?;
//!     Ok(())
//! }
//! ```

mod error;
mod job;
mod queue;
mod storage;
mod storage_list;

pub use error::JobQueueError;
pub use job::Job;
pub use queue::{JobQueue, JobQueueBuilder};
pub use storage::{MemoryStorage, Storage};
pub use storage_list::StorageList;
