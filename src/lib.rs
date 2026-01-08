//! A storage-agnostic async job queue with configurable workers.
//!
//! This crate provides a generic job queue that can work with any storage backend.
//! Built-in implementations are provided:
//!
//! - [`MemoryStorage`] - In-memory queue for testing
//! - Postgres (with `postgres` feature) - Use [`impl_diesel_fetch_and_claim!`] macro
//!   with your own pool type
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │   Storage   │◄────│   JobQueue  │────►│    Job      │
//! │  (YourPool, │     │  (Workers)  │     │  (Your Type)│
//! │  Memory...) │     └─────────────┘     └─────────────┘
//! └─────────────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust
//! use std::convert::Infallible;
//! use fast_job_queue::{Job, JobQueue, JobQueueConfig, MemoryStorage};
//!
//! // Define your job type
//! struct MyJob { id: u64, data: String }
//!
//! // Implement the Job trait
//! impl Job<MemoryStorage<MyJob>> for MyJob {
//!     type Error = Infallible;
//!
//!     async fn fetch_and_claim(
//!         storage: &MemoryStorage<MyJob>
//!     ) -> Result<Option<Self>, Infallible> {
//!         Ok(storage.pop().await)
//!     }
//!
//!     async fn execute(self, _storage: &MemoryStorage<MyJob>) {
//!         println!("Processing job {}: {}", self.id, self.data);
//!     }
//! }
//!
//! # tokio_test::block_on(async {
//! // Create storage and queue
//! let storage = MemoryStorage::new();
//! storage.push(MyJob { id: 1, data: "task".into() }).await;
//!
//! let config = JobQueueConfig::default();
//! let queue = JobQueue::new::<MyJob, _>(storage, config).unwrap();
//!
//! // Queue processes jobs in background...
//! tokio::time::sleep(std::time::Duration::from_millis(100)).await;
//!
//! queue.shutdown().await.unwrap();
//! # });
//! ```
//!
//! # Feature Flags
//!
//! - `postgres` (default) - Enables diesel-async support and the
//!   [`impl_diesel_fetch_and_claim!`] macro for Postgres-based job queues

mod job;
mod queue;
mod storage;

pub use job::Job;
pub use queue::{JobQueue, JobQueueConfig, JobQueueError};
pub use storage::MemoryStorage;
