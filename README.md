# fast-job-queue

A storage-agnostic async job queue with configurable workers for Rust.

## Features

- **Storage Agnostic** - Works with any `Clone + Send + Sync + 'static` type
- **Async Workers** - Spawns configurable number of concurrent worker tasks
- **Postgres Support** - Built-in diesel macro support (feature-gated)
- **Memory Storage** - In-memory implementation for testing
- **Graceful Shutdown** - Clean worker termination with job completion

## Quick Start

```rust
use fast_job_queue::{Job, JobQueue, JobQueueConfig, MemoryStorage, Storage};
use std::convert::Infallible;

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
    let queue = JobQueue::new(JobQueueConfig::default(), storage.clone()).unwrap();

    // Push jobs through storage
    storage.push(EmailJob { to: "user@example.com".into(), body: "Hello!".into() }).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    queue.shutdown().await.unwrap();
}
```

## With Postgres

```rust,ignore
use fast_job_queue::{impl_diesel_pop, Job, JobQueue, JobQueueConfig, Storage};
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
            self.pool,
            get_connection,
            schema::jobs::table,
            schema::jobs::id,
            schema::jobs::status,
            JobStatus::Pending => JobStatus::Running,
            MyJob,
            Uuid,
            MyDbError
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
├── lib.rs          # Re-exports
├── job.rs          # Job<S> trait
├── queue.rs        # JobQueue<S>, JobQueueConfig, JobQueueError
└── storage/
    ├── mod.rs      # Storage trait, MemoryStorage re-export
    ├── memory.rs   # MemoryStorage (in-memory)
    └── postgres.rs # impl_diesel_pop! macro
```

## Feature Flags

| Feature    | Default | Description                      |
| ---------- | ------- | -------------------------------- |
| `postgres` | ✓       | Enables `impl_diesel_pop!` macro |

To use without postgres:

```toml
[dependencies]
fast-job-queue = { version = "0.1", default-features = false }
```

## Configuration

```rust
use std::time::Duration;
use fast_job_queue::JobQueueConfig;

let config = JobQueueConfig {
    workers: 8,                              // Number of concurrent workers
    poll_interval: Duration::from_millis(100), // Polling frequency when idle
};

// Or use defaults (4 workers, 1s poll interval)
let config = JobQueueConfig::default();
```

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).
