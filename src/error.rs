//! Error types for the job queue crate.

use thiserror::Error;

/// Errors that can occur when using the job queue.
#[derive(Debug, Error)]
pub enum JobQueueError {
    /// Queue requires at least one worker.
    #[error("Invalid configuration: workers must be greater than 0")]
    WorkersMustBePositive,

    /// Queue requires a non-zero poll interval.
    #[error("Invalid configuration: poll_interval must be greater than 0")]
    PollIntervalMustBePositive,

    /// Queue requires at least one configured storage backend.
    #[error("Invalid configuration: at least one storage must be configured")]
    MissingStorages,

    /// A worker panicked during execution.
    #[error("Worker panicked: {reason}")]
    WorkerPanicked { reason: String },
}
