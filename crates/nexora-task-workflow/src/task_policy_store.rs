use crate::TaskWorkflowStore;
use nexora_platform::*;
use postgres::Transaction;
use serde_json::Value;

pub(crate) struct TaskPolicyStore;
impl TaskPolicyStore {
    pub fn flow(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
        fresh: bool,
    ) -> Result<WorkflowVersion, PlatformError> {
        crate::task_pool_workflow::TaskPoolWorkflow::flow(tx, Some(project), id, fresh)
    }

    pub fn task(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
    ) -> Result<WorkTask, PlatformError> {
        Self::scoped_task(tx, Some(project), id)
    }
    pub fn scoped_task(
        tx: &mut Transaction<'_>,
        project: Option<&str>,
        id: &str,
    ) -> Result<WorkTask, PlatformError> {
        if let Some(project) = project {
            crate::task_dependency_lock::TaskDependencyLock::shared(tx, project)?;
        }
        let value: Value = tx
            .query_opt(
                "SELECT to_jsonb(t) FROM tasks t WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 FOR UPDATE",
                &[&id, &project],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        TaskWorkflowStore::decode(value)
    }
}
