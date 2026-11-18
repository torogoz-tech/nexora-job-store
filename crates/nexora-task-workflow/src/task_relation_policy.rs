use crate::TaskWorkflowStore;
use nexora_platform::*;
use postgres::Transaction;
pub(crate) struct TaskRelationPolicy;
impl TaskRelationPolicy {
    pub fn blocking_count(tx: &mut Transaction<'_>, task: &str) -> Result<i64, PlatformError> {
        Ok(tx.query_one("SELECT count(*) FROM task_dependencies d JOIN tasks b ON b.id=CASE WHEN d.kind='depends_on' THEN d.target_task_id ELSE d.source_task_id END WHERE d.status='active' AND d.removed_at IS NULL AND ((d.kind='depends_on' AND d.source_task_id=$1) OR(d.kind='blocks' AND d.target_task_id=$1)) AND b.lifecycle<>'completed'",&[&task]).map_err(TaskWorkflowStore::database_error)?.get(0))
    }
    pub fn unfinished_children(tx: &mut Transaction<'_>, task: &str) -> Result<i64, PlatformError> {
        Ok(tx.query_one("SELECT count(*) FROM task_subtasks s JOIN tasks c ON c.id=s.child_task_id JOIN tasks p ON p.id=s.parent_task_id WHERE s.parent_task_id=$1 AND s.removed_at IS NULL AND c.lifecycle<>'completed' AND (NOT p.governed_stage OR ((s.stage_workflow_version_id IS NULL OR s.stage_workflow_version_id=p.workflow_version_id) AND NOT EXISTS(SELECT 1 FROM task_stage_completions done WHERE done.task_id=c.id AND done.workflow_version_id=COALESCE(s.stage_workflow_version_id,p.workflow_version_id) AND c.workflow_version_id=done.workflow_version_id AND EXISTS(SELECT 1 FROM workflow_versions cv,jsonb_array_elements(cv.graph->'states') cs WHERE cv.id=c.workflow_version_id AND cs->>'id'=c.state_id AND COALESCE((cs->>'terminal')::boolean,false)))))",&[&task]).map_err(TaskWorkflowStore::database_error)?.get(0))
    }
    pub fn state(
        tx: &mut Transaction<'_>,
        task: &WorkTask,
        terminal: bool,
    ) -> Result<(), PlatformError> {
        if terminal && Self::unfinished_children(tx, &task.id)? > 0 {
            return Err(PlatformError::Conflict(
                "complete all subtasks before completing this task",
            ));
        }
        if !terminal && tx.query_one("SELECT EXISTS(SELECT 1 FROM task_subtasks s JOIN tasks p ON p.id=s.parent_task_id WHERE s.child_task_id=$1 AND s.removed_at IS NULL AND p.lifecycle='completed')",&[&task.id]).map_err(TaskWorkflowStore::database_error)?.get::<_,bool>(0){return Err(PlatformError::Conflict("cannot reopen subtask of completed parent"));}
        Ok(())
    }
}
