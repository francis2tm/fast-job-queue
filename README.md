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
use fast_job_queue::{Job, JobQueue, JobQueueConfig, MemoryStorage};
use std::convert::Infallible;

struct EmailJob { to: String, body: String }

impl Job<MemoryStorage<EmailJob>> for EmailJob {
    type Error = Infallible;

    async fn fetch_and_claim(storage: &MemoryStorage<EmailJob>) -> Result<Option<Self>, Infallible> {
        Ok(storage.pop().await)
    }

    async fn execute(self, _storage: &MemoryStorage<EmailJob>) {
        println!("Sending email to {}", self.to);
    }
}

#[tokio::main]
async fn main() {
    let storage = MemoryStorage::new();
    storage.push(EmailJob { to: "user@example.com".into(), body: "Hello!".into() }).await;

    let queue = JobQueue::new::<EmailJob, _>(storage, JobQueueConfig::default()).unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    queue.shutdown().await.unwrap();
}
```

## With Postgres

```rust,ignore
use fast_job_queue::{impl_diesel_fetch_and_claim, Job, JobQueue, JobQueueConfig};

impl Job<MyDbPool> for MyJob {
    type Error = MyDbError;

    async fn fetch_and_claim(pool: &MyDbPool) -> Result<Option<Self>, MyDbError> {
        impl_diesel_fetch_and_claim!(
            pool: pool,
            get_connection: get_connection,
            table: schema::jobs::table,
            id_column: schema::jobs::id,
            status_column: schema::jobs::status,
            pending_status: "pending",
            running_status: "running",
            model: MyJob,
            id_type: Uuid,
            error_type: MyDbError
        )
    }

    async fn execute(self, pool: &MyDbPool) {
        // Process job, update status to completed/failed
    }
}
```

## Architecture

```
src/
├── lib.rs          # Re-exports
├── job.rs          # Job trait (with Error associated type)
├── queue.rs        # JobQueue, JobQueueConfig, JobQueueError
└── storage/
    ├── mod.rs      # MemoryStorage re-export
    ├── memory.rs   # MemoryStorage (in-memory)
    └── postgres.rs # fetch_and_claim macro
```

## Feature Flags

| Feature    | Default | Description                                  |
| ---------- | ------- | -------------------------------------------- |
| `postgres` | ✓       | Enables `impl_diesel_fetch_and_claim!` macro |

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
