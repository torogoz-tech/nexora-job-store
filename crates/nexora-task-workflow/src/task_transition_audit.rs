use nexora_platform::{AuthContext, PlatformError, WorkTask};
use postgres::Transaction;
use serde_json::{Value, json};

/// Writes task history and its matching audit event in one existing-schema transaction.
pub(crate) struct TaskTransitionAudit;

impl TaskTransitionAudit {
    pub(crate) fn event(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        task: &WorkTask,
        operation: &str,
        details: Value,
    ) -> Result<(), PlatformError> {
        let sequence: i64 = transaction
            .query_one(
                "UPDATE tasks SET history_sequence=history_sequence+1 WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 RETURNING history_sequence",
                &[&task.id, &task.project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .get(0);
        let history_id = crate::task_workflow_secret_id::TaskWorkflowSecretId::generate("the")?;
        transaction
            .execute(
                "INSERT INTO task_history(id,project_id,task_id,sequence,revision,operation,actor_id,credential_id,details) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                &[&history_id, &task.project_id, &task.id, &sequence, &task.revision, &operation, &context.actor_id(), &context.credential_id(), &details],
            )
            .map_err(|_| PlatformError::Storage)?;
        let audit_id = crate::task_workflow_secret_id::TaskWorkflowSecretId::generate("evt")?;
        let metadata = json!({
            "project_id": task.project_id,
            "revision": task.revision,
            "sequence": sequence,
            "approval_id": details.get("approval_id"),
            "reason": details.get("reason"),
        });
        transaction
            .execute(
                "INSERT INTO audit_events(id,actor_id,credential_id,operation,resource_id,metadata) VALUES($1,$2,$3,$4,$5,$6)",
                &[&audit_id, &Some(context.actor_id()), &Some(context.credential_id()), &operation, &Some(task.id.as_str()), &metadata],
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(())
    }
}
