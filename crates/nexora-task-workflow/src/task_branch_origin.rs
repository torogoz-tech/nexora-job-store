use crate::TaskWorkflowStore;
use nexora_platform::*;
/// Resolves hierarchical branch origins from persisted task relationships, never labels.
pub(crate) struct TaskBranchOrigin;
impl TaskBranchOrigin {
    pub fn resolve(
        store: &mut TaskWorkflowStore,
        task: &str,
        repository: &str,
        configured: &str,
        sha: &str,
    ) -> Result<(String, String), PlatformError> {
        let row=store.client().query_opt("SELECT s.work_kind,s.tested_repository_id,s.tested_commit_sha,b.branch_name,b.head_sha,(child.workflow_version_id=parent.workflow_version_id),b.evidence,parent.workflow_version_id FROM task_subtasks s JOIN tasks child ON child.id=s.child_task_id JOIN tasks parent ON parent.id=s.parent_task_id LEFT JOIN task_repository_branches b ON b.task_id=s.parent_task_id AND b.repository_id=$2 WHERE s.child_task_id=$1 AND s.removed_at IS NULL",&[&task,&repository]).map_err(TaskWorkflowStore::database_error)?;
        let Some(row) = row else {
            return Ok((configured.into(), sha.into()));
        };
        if row.get::<_, String>(0) == "corrective" {
            if row.get::<_, Option<bool>>(5) == Some(true) {
                return Ok((configured.into(), sha.into()));
            }
            if row.get::<_, Option<String>>(1).as_deref() != Some(repository) {
                return Err(PlatformError::Conflict(
                    "corrective branch must use the repository revision tested by the parent",
                ));
            }
            let tested: String = row
                .get::<_, Option<String>>(2)
                .ok_or(PlatformError::Conflict("tested revision is missing"))?;
            return Ok((tested.clone(), tested));
        }
        let evidence: serde_json::Value =
            row.get::<_, Option<serde_json::Value>>(6)
                .ok_or(PlatformError::Conflict(
                    "create the parent task branch before creating a subtask branch",
                ))?;
        if evidence["provenance"] != "provider_verified"
            || evidence["workflow_version_id"]
                .as_str()
                .is_some_and(|version| Some(version) != row.get::<_, Option<String>>(7).as_deref())
        {
            return Err(PlatformError::Conflict(
                "take and verify the parent task's current workflow branch before creating a decomposition branch",
            ));
        }
        Ok((
            row.get::<_, Option<String>>(3)
                .ok_or(PlatformError::Conflict(
                    "create the parent task branch before creating a subtask branch",
                ))?,
            row.get::<_, Option<String>>(4)
                .ok_or(PlatformError::Conflict("parent branch revision is missing"))?,
        ))
    }
    pub fn save(
        store: &mut TaskWorkflowStore,
        task: &str,
        branch: &TaskTransitionBranch,
        base: &str,
        sha: &str,
        evidence: &serde_json::Value,
    ) -> Result<(), PlatformError> {
        let count=store.client().execute("INSERT INTO task_repository_branches(task_id,repository_id,branch_name,base_branch,base_sha,head_sha,evidence) VALUES($1,$2,$3,$4,$5,$5,$6) ON CONFLICT(task_id,repository_id) DO UPDATE SET evidence=EXCLUDED.evidence WHERE task_repository_branches.branch_name=EXCLUDED.branch_name AND task_repository_branches.base_sha=EXCLUDED.base_sha",&[&task,&branch.repository_id,&branch.branch_name,&base,&sha,evidence]).map_err(TaskWorkflowStore::database_error)?;
        if count != 1 {
            return Err(PlatformError::Conflict(
                "a different task branch is already registered for this repository",
            ));
        }
        Ok(())
    }
}
