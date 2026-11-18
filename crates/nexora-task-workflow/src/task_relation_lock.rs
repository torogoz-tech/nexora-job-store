use crate::TaskWorkflowStore;
use nexora_platform::PlatformError;
use postgres::Transaction;
pub(crate) struct TaskRelationLock;
impl TaskRelationLock {
    pub fn acquire(tx: &mut Transaction<'_>) -> Result<(), PlatformError> {
        tx.query_one("SELECT task_relations_lock()", &[])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(())
    }
}
