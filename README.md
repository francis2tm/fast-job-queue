# fast-job-queue

A storage-agnostic async job queue with configurable workers for Rust.

## Features

- **Multi-Storage** - Single queue can process jobs from multiple storage backends
- **Storage Agnostic** - Works with any `Send + Sync + 'static` storage backend
- **Async Workers** - Spawns configurable number of concurrent worker tasks
- **Postgres Support** - Built-in diesel macro support (feature-gated)
- **Memory Storage** - In-memory implementation for testing
- **Graceful Shutdown** - Clean worker termination with job completion

## Quick Start

```rust
use fast_job_queue::{Job, JobQueue, MemoryStorage, Storage};
use std::convert::Infallible;
use std::time::Duration;

struct EmailJob { to: String, body: String }

impl Job<MemoryStorage<EmailJob>> for EmailJob {
    type Error = Infallible;

    async fn execute(self, _storage: &MemoryStorage<EmailJob>) -> Result<(), Infallible> {
        println!("Sending email to {}", self.to);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let storage = MemoryStorage::new();

    let queue = JobQueue::builder()
        .workers(4)
        .poll_interval(Duration::from_secs(1))
        .with_storage(storage.clone())
        .build()
        .unwrap();

    // Push jobs through storage
    storage.push(EmailJob { to: "user@example.com".into(), body: "Hello!".into() }).await.unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    queue.shutdown().await.unwrap();
}
```

## Multi-Storage Queue

A single queue can poll jobs from multiple storage backends:

```rust,ignore
let queue = JobQueue::builder()
    .workers(4)
    .poll_interval(Duration::from_millis(100))
    .with_storage(transcription_storage)  // Postgres-backed
    .with_storage(translation_storage)    // Postgres-backed
    .with_storage(email_storage)          // Memory-backed
    .build()?;

// Single shutdown stops all workers
queue.shutdown().await?;
```

Workers poll all storages and process the first available job.

## With Postgres

```rust,ignore
use fast_job_queue::{impl_diesel_pop, Job, JobQueue, Storage};
use std::convert::Infallible;

// Your storage wraps a database pool
#[derive(Clone)]
struct MyStorage { pool: MyDbPool }

impl Storage for MyStorage {
    type Job = MyJob;
    type Error = MyDbError;

    async fn push(&self, _job: Self::Job) -> Result<(), Self::Error> {
        // Insert job into database or unimplemented if using API handlers
        unimplemented!("Use API handlers to insert jobs")
    }

    async fn pop(&self) -> Result<Option<Self::Job>, Self::Error> {
        impl_diesel_pop!(
            pool: self.pool,
            get_connection: get_connection,
            table: schema::jobs::table,
            id_column: schema::jobs::id,
            status_column: schema::jobs::status,
            pending_status: JobStatus::Pending,
            running_status: JobStatus::Running,
            model: MyJob,
            id_type: Uuid,
            error_type: MyDbError,
        )
    }
}

impl Job<MyStorage> for MyJob {
    type Error = Infallible;

    async fn execute(self, storage: &MyStorage) -> Result<(), Infallible> {
        // Process job, update status to completed/failed
        Ok(())
    }
}
```

## Architecture

```
src/
├── lib.rs           # Re-exports
├── job.rs           # Job<S> trait
├── queue.rs         # JobQueueBuilder, JobQueue
├── storage_list.rs  # StorageList trait (tuple composition)
└── storage/
    ├── mod.rs       # Storage trait, MemoryStorage re-export
    ├── memory.rs    # MemoryStorage (in-memory)
    └── postgres.rs  # impl_diesel_pop! macro
```

## Feature Flags

| Feature    | Default | Description                      |
| ---------- | ------- | -------------------------------- |
| `postgres` |         | Enables `impl_diesel_pop!` macro |

To use with postgres:

```toml
[dependencies]
fast-job-queue = { version = "0.1", features = ["postgres"] }
```

## Configuration

```rust
use std::time::Duration;
use fast_job_queue::JobQueue;

let queue = JobQueue::builder()
    .workers(8)
    .poll_interval(Duration::from_millis(100))
    .with_storage(my_storage)
    .build()?;
```

Call `queue.shutdown().await` for graceful shutdown when your service is stopping.
