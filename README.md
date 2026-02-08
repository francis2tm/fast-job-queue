# fast-job-queue

A storage-agnostic async job queue with configurable workers for Rust.

## Features

- **Runtime Multi-Storage** - Add any number of storages at runtime
- **Typed Storage** - Each storage is generic over one static job type
- **Async Workers** - Spawns configurable concurrent worker tasks
- **Graceful Shutdown** - Consuming shutdown waits for workers
- **Postgres Helper** - `impl_diesel_pop!` macro (feature-gated)
- **Memory Storage** - In-memory storage for testing and simple cases

## Quick Start

```rust
use fast_job_queue::{Job, JobQueue, MemoryStorage, StorageError};
use std::time::Duration;

struct EmailJob;

impl Job for EmailJob {
    async fn execute(self) -> Result<(), StorageError> {
        println!("sending email");
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let storage = MemoryStorage::<EmailJob>::new();

    let queue = JobQueue::builder()
        .workers(4)
        .poll_interval(Duration::from_secs(1))
        .with_storage(storage.clone())
        .build()
        .unwrap();

    storage.job_push(EmailJob).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    queue.shutdown().await.unwrap();
}
```

## E2E: Service Wrapper Lifecycle

```rust,no_run
use fast_job_queue::{Job, JobQueue, JobQueueError, MemoryStorage, StorageError};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

struct EmailJob {
    to: String,
}

impl Job for EmailJob {
    async fn execute(self) -> Result<(), StorageError> {
        println!("sent email to {}", self.to);
        Ok(())
    }
}

#[derive(Clone)]
struct EmailService {
    storage: MemoryStorage<EmailJob>,
    queue: Arc<Mutex<Option<JobQueue>>>,
}

impl EmailService {
    fn service_build() -> Self {
        let storage = MemoryStorage::new();
        let queue = JobQueue::builder()
            .workers(2)
            .poll_interval(Duration::from_millis(100))
            .with_storage(storage.clone())
            .build()
            .unwrap();

        Self {
            storage,
            queue: Arc::new(Mutex::new(Some(queue))),
        }
    }

    async fn email_enqueue(&self, to: String) {
        self.storage.job_push(EmailJob { to }).await;
    }

    async fn service_shutdown(&self) -> Result<(), JobQueueError> {
        match self.queue.lock().await.take() {
            Some(queue) => queue.shutdown().await,
            None => Ok(()),
        }
    }
}
```

## E2E: One Queue, Multiple Job Types

```rust,no_run
use fast_job_queue::{Job, JobQueue, MemoryStorage, StorageError};
use std::time::Duration;

struct EmailJob;
struct CleanupJob;

impl Job for EmailJob {
    async fn execute(self) -> Result<(), StorageError> {
        println!("email sent");
        Ok(())
    }
}

impl Job for CleanupJob {
    async fn execute(self) -> Result<(), StorageError> {
        println!("cleanup completed");
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let email_storage = MemoryStorage::<EmailJob>::new();
    let cleanup_storage = MemoryStorage::<CleanupJob>::new();

    let queue = JobQueue::builder()
        .workers(4)
        .poll_interval(Duration::from_millis(100))
        .with_storage(email_storage.clone())
        .with_storage(cleanup_storage.clone())
        .build()
        .unwrap();

    email_storage.job_push(EmailJob).await;
    cleanup_storage.job_push(CleanupJob).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    queue.shutdown().await.unwrap();
}
```

## E2E: Postgres Storage + Worker Queue

Enable the `postgres` feature and use `impl_diesel_pop!` inside your storage implementation.

```rust,ignore
use fast_job_queue::{impl_diesel_pop, Job, JobQueue, Storage, StorageError};
use uuid::Uuid;

struct MyJob(JobRow);

impl Job for MyJob {
    async fn execute(self) -> Result<(), StorageError> {
        process_job(self.0).await
    }
}

impl Storage<MyJob> for MyStorage {
    async fn job_pop(&self) -> Result<Option<MyJob>, StorageError> {
        let maybe_job = impl_diesel_pop!(
            self.pool.clone(),
            get_connection,
            schema::jobs::table,
            schema::jobs::id,
            schema::jobs::status,
            JobStatus::Pending => JobStatus::Running,
            Job,
            Uuid,
            MyError
        )?;

        Ok(maybe_job.map(MyJob::from))
    }
}

#[tokio::main]
async fn main() -> Result<(), fast_job_queue::JobQueueError> {
    let queue = JobQueue::builder()
        .workers(8)
        .poll_interval(std::time::Duration::from_millis(100))
        .with_storage(MyStorage::new(db_pool))
        .build()?;

    queue.shutdown().await
}
```

## Feature Flags

| Feature    | Default | Description            |
| ---------- | ------- | ---------------------- |
| `postgres` |         | Enables `impl_diesel_pop!` |
