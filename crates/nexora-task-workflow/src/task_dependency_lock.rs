use crate::TaskWorkflowStore;
use nexora_platform::PlatformError;
use postgres::Transaction;

/// Project graph authority is acquired before task rows, for readers and writers alike.
pub(crate) struct TaskDependencyLock;
impl TaskDependencyLock {
    pub fn shared(tx: &mut Transaction<'_>, project: &str) -> Result<(), PlatformError> {
        tx.query_one("SELECT pg_advisory_xact_lock_shared(hashtextextended(current_schema() || ':task-dependencies:' || $1::text,0))",&[&project]).map_err(TaskWorkflowStore::database_error)?;
        Ok(())
    }
}
