use crate::{
    TaskWorkflowStore, development_policy_store::DevelopmentPolicyStore,
    repository_access::RepositoryAccess,
};
use nexora_platform::*;
use serde_json::json;
pub(crate) struct TaskWorkStore;
impl TaskWorkStore {
    pub fn session(
        tx: &mut postgres::Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        task: &str,
        id: &str,
    ) -> Result<TaskWorkSession, PlatformError> {
        let row=tx.query_opt("SELECT record,task_revision,workflow_version_id,transition_id,reported FROM task_work_sessions WHERE id=$1 AND project_id=$2 AND task_id=$3 AND actor_id=$4 FOR UPDATE",&[&id,&project,&task,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
        let session: TaskWorkSession = TaskWorkflowStore::decode(row.get(0))?;
        if session.id != id
            || session.project_id != project
            || session.task_id != task
            || session.actor_id != ctx.actor_id()
            || session.task_revision != row.get::<_, i64>(1)
            || session.workflow_version_id != row.get::<_, String>(2)
            || session.transition_id != row.get::<_, String>(3)
            || session.report.is_some() != row.get::<_, bool>(4)
            || session.branches.is_empty()
        {
            return Err(PlatformError::Storage);
        }
        Ok(session)
    }
    pub fn source_authority(
        tx: &mut postgres::Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
    ) -> Result<serde_json::Value, PlatformError> {
        let source = crate::repository_snapshot_scope::RepositorySnapshotScope::lock(
            tx, ctx, project, repository, None,
        )?;
        let mut authority = source.authority(tx)?;
        let row=tx.query_one("SELECT host,remote_full_name FROM project_repositories WHERE project_id=$1 AND id=$2",&[&project,&repository]).map_err(TaskWorkflowStore::database_error)?;
        authority["host"] = json!(row.get::<_, Option<String>>(0));
        authority["remote_full_name"] = json!(row.get::<_, Option<String>>(1));
        Ok(authority)
    }
    pub fn policies(
        tx: &mut postgres::Transaction<'_>,
        ctx: &AuthContext,
        session: &TaskWorkSession,
    ) -> Result<(), PlatformError> {
        let flow = crate::task_pool_workflow::TaskPoolWorkflow::flow(
            tx,
            Some(&session.project_id),
            &session.workflow_version_id,
            false,
        )?;
        let state_base = flow
            .graph
            .transitions
            .iter()
            .find(|edge| edge.id == session.transition_id)
            .and_then(|edge| flow.graph.states.iter().find(|state| state.id == edge.from))
            .and_then(|state| state.required_base_branch.as_deref());
        let governed: bool = tx
            .query_one(
                "SELECT governed_stage FROM tasks WHERE id=$1 AND project_id=$2",
                &[&session.task_id, &session.project_id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        for branch in &session.branches {
            if Self::source_authority(tx, ctx, &session.project_id, &branch.repository_id)?
                != branch.source_authority
            {
                return Err(PlatformError::Conflict("repository work authority changed"));
            }
            RepositoryAccess::lock(tx, ctx, &session.project_id, &branch.repository_id)?;
            let policy =
                DevelopmentPolicyStore::lock(tx, &session.project_id, &branch.repository_id)?;
            let origin_matches = if governed {
                tx.query_opt("SELECT branch_name,base_branch,base_sha FROM task_repository_branches WHERE task_id=$1 AND repository_id=$2",&[&session.task_id,&branch.repository_id]).map_err(TaskWorkflowStore::database_error)?.is_some_and(|row|row.get::<_,String>(0)==branch.branch_name&&row.get::<_,String>(1)==branch.base_branch&&Some(row.get::<_,String>(2)).as_ref()==branch.base_sha.as_ref())
            } else {
                state_base.unwrap_or(&policy.base_branch) == branch.base_branch
            };
            if policy.revision != branch.policy_revision
                || !origin_matches
                || policy.branch_prefix != branch.branch_prefix
            {
                return Err(PlatformError::Conflict("development policy changed"));
            }
        }
        Ok(())
    }
    pub fn condition(
        tx: &mut postgres::Transaction<'_>,
        ctx: &AuthContext,
        task: &WorkTask,
        transition: &str,
    ) -> Result<bool, PlatformError> {
        let id:Option<String>=tx.query_opt("SELECT id FROM task_work_sessions WHERE project_id=$1 AND task_id=$2 AND actor_id=$3 AND task_revision=$4 AND workflow_version_id=$5 AND transition_id=$6 AND reported ORDER BY created_at DESC,id DESC LIMIT 1 FOR SHARE",&[&task.project_id,&task.id,&ctx.actor_id(),&task.revision,&task.workflow_version_id,&transition]).map_err(TaskWorkflowStore::database_error)?.map(|r|r.get(0));
        let Some(id) = id else { return Ok(false) };
        let session = Self::session(tx, ctx, task.project_scope()?, &task.id, &id)?;
        Self::policies(tx, ctx, &session)?;
        Ok(session.branches.iter().all(|b| {
            b.base_sha.as_ref().is_some_and(|s| {
                matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit())
            }) && b
                .head_sha
                .as_ref()
                .is_some_and(|s| TaskWorkBranchReport::valid_sha(s))
        }))
    }
}
