use crate::{
    TaskWorkflowStore, task_approval_fingerprint::TaskApprovalFingerprint,
    task_history_writer::TaskHistoryWriter,
};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::{Value, json};

pub(crate) struct TaskApprovalStore;
impl TaskApprovalStore {
    pub fn consume(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        task: &WorkTask,
        flow: &WorkflowVersion,
        transition_id: &str,
        input: &TaskTransitionRequest,
    ) -> Result<bool, PlatformError> {
        let Some(id) = &input.approval_id else {
            return Ok(false);
        };
        let transition = flow
            .graph
            .transitions
            .iter()
            .find(|edge| {
                edge.id == transition_id && task.state_id.as_deref() == Some(edge.from.as_str())
            })
            .ok_or(PlatformError::Validation("transition is not available"))?;
        if !transition.requires_approval() {
            return Err(PlatformError::Validation(
                "transition does not accept an approval",
            ));
        }
        let value:Value=tx.query_opt("SELECT to_jsonb(a) FROM task_approvals a WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 AND task_id=$3 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE",&[&id,&task.project_id,&task.id]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("approval is unavailable"))?.get(0);
        let approval: TaskApproval = TaskWorkflowStore::decode(value)?;
        if transition.requires_approval() && approval.granted_by == ctx.actor_id() {
            return Err(PlatformError::Forbidden);
        }
        if approval.task_revision != task.revision
            || approval.workflow_version_id != flow.id
            || approval.workflow_id != flow.workflow_id
            || approval.transition_id != transition.id
            || task.state_id.as_deref() != Some(approval.from_state.as_str())
            || approval.to_state != transition.to
            || approval.parameters_checksum
                != TaskApprovalFingerprint::checksum(task, transition, input)?
        {
            return Err(PlatformError::Conflict(
                "approval does not match this operation",
            ));
        }
        tx.query_opt(
            "SELECT id FROM actors WHERE id=$1 AND active AND kind='human' FOR SHARE",
            &[&approval.granted_by],
        )
        .map_err(TaskWorkflowStore::database_error)?
        .ok_or(PlatformError::Conflict("approval grantor is inactive"))?;
        let updated=tx.execute("UPDATE task_approvals SET consumed_at=clock_timestamp(),consumed_by=$2,consumed_credential_id=$3 WHERE id=$1 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",&[&id,&ctx.actor_id(),&ctx.credential_id()]).map_err(TaskWorkflowStore::database_error)?;
        if updated != 1 {
            return Err(PlatformError::Conflict("approval expired"));
        }
        TaskHistoryWriter::event(
            tx,
            ctx,
            task,
            "task.approval_consumed",
            json!({"approval_id":id,"transition_id":transition.id,"parameters_checksum":approval.parameters_checksum}),
        )?;
        Ok(true)
    }
}
