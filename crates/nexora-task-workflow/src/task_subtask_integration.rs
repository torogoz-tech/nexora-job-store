use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::*;
/// A finished code subtask must actually be integrated before its parent can finish the stage.
pub(crate) struct TaskSubtaskIntegration;
impl TaskSubtaskIntegration {
    pub fn verify(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        task: &WorkTask,
        policy: &GitNetworkPolicy,
    ) -> Result<(), PlatformError> {
        if !task.governed_stage || task.project_id.is_none() {
            return Ok(());
        }
        let rows=store.client().query("SELECT c.id,c.purpose,b.repository_id,b.branch_name,b.head_sha,p.branch_name FROM task_subtasks s JOIN tasks c ON c.id=s.child_task_id LEFT JOIN task_repository_branches b ON b.task_id=c.id LEFT JOIN task_repository_branches p ON p.task_id=s.parent_task_id AND p.repository_id=b.repository_id WHERE s.parent_task_id=$1 AND s.removed_at IS NULL AND s.work_kind='decomposition' AND s.stage_workflow_version_id=$2",&[&task.id,&task.workflow_version_id]).map_err(TaskWorkflowStore::database_error)?;
        for row in rows {
            let repository: Option<String> = row.get(2);
            let Some(repository) = repository else {
                if row.get::<_, String>(1) == "implementation" {
                    return Err(PlatformError::Conflict(
                        "implementation subtask needs verified repository branch integration before the parent can finish",
                    ));
                }
                continue;
            };
            let target: String = row
                .get::<_, Option<String>>(5)
                .ok_or(PlatformError::Conflict(
                    "parent task branch is missing for subtask integration",
                ))?;
            let branch = TaskTransitionBranch {
                repository_id: repository,
                branch_name: row.get(3),
                expected_head_sha: row.get(4),
            };
            crate::task_transition_provider::TaskTransitionProvider::observe_to(
                crate::provider_request_context::ProviderRequestContext {
                    store,
                    ctx,
                    project: task.project_id.as_deref(),
                    policy,
                },
                crate::provider_observation_request::ProviderObservationRequest {
                    branch: &branch,
                    observation: "pull_request_merged",
                    selector: None,
                    target_branch: Some(&target),
                },
            )?;
        }
        Ok(())
    }
}
