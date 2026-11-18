use crate::TaskWorkflowStore;
use nexora_platform::*;
/// Explicit runtime selectors. Literal Git branch names are never reinterpreted.
pub(crate) struct TaskBranchSelectors;
impl TaskBranchSelectors {
    pub fn source(
        store: &mut TaskWorkflowStore,
        task: &WorkTask,
        branch: &TaskTransitionBranch,
        literal: &str,
        from_task: bool,
        release: bool,
    ) -> Result<Option<String>, PlatformError> {
        if !from_task {
            return Ok((branch.branch_name == literal).then(|| literal.to_owned()));
        }
        if release {
            return Ok(Some(branch.branch_name.clone()));
        } // The engine already validates the selected release registration.
        let row=store.client().query_opt("SELECT branch_name,evidence FROM task_repository_branches WHERE task_id=$1 AND repository_id=$2",&[&task.id,&branch.repository_id]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("task source branch is not registered; take repository work first"))?;
        let name: String = row.get(0);
        let evidence: serde_json::Value = row.get(1);
        if evidence["provenance"] != "provider_verified" {
            return Err(PlatformError::Conflict(
                "task source branch requires provider-verified registration",
            ));
        }
        if evidence["workflow_version_id"]
            .as_str()
            .is_some_and(|version| Some(version) != task.workflow_version_id.as_deref())
        {
            return Err(PlatformError::Conflict(
                "task branch belongs to an earlier workflow stage",
            ));
        }
        Ok((branch.branch_name == name).then_some(name))
    }
    pub fn target(
        store: &mut TaskWorkflowStore,
        task: &WorkTask,
        repository: &str,
        literal: Option<&str>,
        from_parent: bool,
    ) -> Result<Option<String>, PlatformError> {
        if !from_parent {
            return Ok(literal.map(str::to_owned));
        }
        let row=store.client().query_opt("SELECT b.branch_name,b.evidence,parent.workflow_version_id FROM task_subtasks s JOIN tasks parent ON parent.id=s.parent_task_id AND NOT parent.archived JOIN task_repository_branches b ON b.task_id=parent.id AND b.repository_id=$2 WHERE s.child_task_id=$1 AND s.removed_at IS NULL AND s.stage_workflow_version_id IS NOT DISTINCT FROM parent.workflow_version_id",&[&task.id,&repository]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("parent task branch is not registered in the originating workflow stage"))?;
        let evidence: serde_json::Value = row.get(1);
        if evidence["provenance"] != "provider_verified" {
            return Err(PlatformError::Conflict(
                "parent target branch requires provider-verified registration",
            ));
        }
        if evidence["workflow_version_id"]
            .as_str()
            .is_some_and(|version| Some(version) != row.get::<_, Option<String>>(2).as_deref())
        {
            return Err(PlatformError::Conflict(
                "take the parent task's current stage before selecting its target branch",
            ));
        }

        Ok(Some(row.get(0)))
    }
}
