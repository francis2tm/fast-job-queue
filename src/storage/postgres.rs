//! Postgres storage backend via diesel.
//!
//! This module provides the [`impl_diesel_pop!`] macro for
//! implementing [`Storage::pop`] with diesel models.

/// Implement the `pop` method for diesel-backed Storage implementations.
///
/// This implements the common pattern of:
/// 1. `SELECT id FROM table WHERE status = pending LIMIT 1 FOR UPDATE SKIP LOCKED`
/// 2. `UPDATE table SET status = running WHERE id = selected_id`
/// 3. `SELECT * FROM table WHERE id = selected_id`
///
/// All operations are performed within a single database transaction to ensure atomicity.
///
/// # Arguments (positional)
///
/// 1. `pool` - Expression yielding the pool reference
/// 2. `get_connection` - Method name to get an async connection from the pool
/// 3. `table` - The diesel table (e.g., `schema::jobs::table`)
/// 4. `id_column` - The ID column (e.g., `schema::jobs::id`)
/// 5. `status_column` - The status column (e.g., `schema::jobs::status`)
/// 6. `pending => running` - Status transition (pending_status => running_status)
/// 7. `model` - The model type to return
/// 8. `id_type` - The ID column's Rust type (e.g., `Uuid`)
/// 9. `error_type` - The error type for Result (must implement `From<diesel::result::Error>`)
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::{impl_diesel_pop, Storage};
///
/// impl Storage for MyStorage {
///     type Job = MyJob;
///     type Error = MyDbError;
///
///     async fn push(&self, _job: Self::Job) -> Result<(), Self::Error> {
///         unimplemented!("Use API to insert jobs")
///     }
///
///     async fn pop(&self) -> Result<Option<Self::Job>, Self::Error> {
///         impl_diesel_pop!(
///             self.db_pool,
///             get_service_connection,
///             schema::jobs::table,
///             schema::jobs::id,
///             schema::jobs::status,
///             JobStatus::Pending => JobStatus::Running,
///             MyJob,
///             Uuid,
///             MyDbError
///         )
///     }
/// }
/// ```
#[macro_export]
macro_rules! impl_diesel_pop {
    (
        $pool:expr,
        $get_conn:ident,
        $table:expr,
        $id_column:expr,
        $status_column:expr,
        $pending_status:expr => $running_status:expr,
        $model:ty,
        $id_type:ty,
        $error_type:ty
    ) => {{
        use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, SelectableHelper, update};
        use diesel_async::{AsyncConnection, RunQueryDsl, scoped_futures::ScopedFutureExt};

        // Get connection using the provided method
        let mut conn = $pool.$get_conn().await?;

        // Fetch a single pending job and mark it as running in a single transaction
        let job: Option<$model> = conn
            .transaction(|conn| {
                async move {
                    // Select a single pending job with FOR UPDATE SKIP LOCKED
                    let pending_id: Option<$id_type> = $table
                        .filter($status_column.eq($pending_status))
                        .select($id_column)
                        .limit(1)
                        .for_update()
                        .skip_locked()
                        .first::<$id_type>(conn)
                        .await
                        .optional()?;

                    let pending_id = match pending_id {
                        Some(id) => id,
                        None => return Ok::<Option<$model>, $error_type>(None),
                    };

                    // Mark as Running atomically before releasing the lock
                    update($table)
                        .filter($id_column.eq(pending_id))
                        .set($status_column.eq($running_status))
                        .execute(conn)
                        .await?;

                    // Fetch and return the now-Running job
                    let job = $table
                        .filter($id_column.eq(pending_id))
                        .select(<$model>::as_select())
                        .first::<$model>(conn)
                        .await?;

                    Ok::<Option<$model>, $error_type>(Some(job))
                }
                .scope_boxed()
            })
            .await?;

        Ok::<Option<$model>, $error_type>(job)
    }};
}
