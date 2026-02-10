//! Postgres helper utilities via diesel.
//!
//! This module provides a macro for the common "claim pending job" flow.

/// Implement the "claim pending id, mark running, load model" flow for diesel-backed queues.
///
/// The generated code executes all steps in one transaction:
/// 1. select one pending id with `FOR UPDATE SKIP LOCKED`
/// 2. mark it as running
/// 3. load and return the full model
///
/// # Example
///
/// ```rust,ignore
/// use fast_job_queue::impl_diesel_pop;
///
/// let job = impl_diesel_pop!(
///     self.db_pool.clone(),
///     get_service_connection,
///     schema::jobs::table,
///     schema::jobs::id,
///     schema::jobs::status,
///     JobStatus::Pending => JobStatus::Running,
///     Job,
///     Uuid,
///     DbError
/// )?;
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

        let mut conn = $pool.$get_conn().await?;

        let job: Option<$model> = conn
            .transaction(|conn| {
                async move {
                    let pending_id: Option<$id_type> = $table
                        .filter($status_column.eq($pending_status))
                        .select($id_column)
                        .limit(1)
                        .for_update()
                        .skip_locked()
                        .first::<$id_type>(conn)
                        .await
                        .optional()?;

                    let Some(pending_id) = pending_id else {
                        return Ok::<Option<$model>, $error_type>(None);
                    };

                    update($table)
                        .filter($id_column.eq(pending_id))
                        .set($status_column.eq($running_status))
                        .execute(conn)
                        .await?;

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
