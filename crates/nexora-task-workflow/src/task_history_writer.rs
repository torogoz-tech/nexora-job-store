use crate::{SecretGenerator, TaskWorkflowStore, audit_writer::AuditWriter};
use nexora_platform::{AuthContext, PlatformError, WorkTask};
use postgres::Transaction;
use serde_json::json;

pub(crate) struct TaskHistoryWriter;
impl TaskHistoryWriter {
    pub fn append(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        task: &WorkTask,
        operation: &str,
        before: Option<&WorkTask>,
        reason: Option<&str>,
    ) -> Result<(), PlatformError> {
        Self::event(
            tx,
            ctx,
            task,
            operation,
            json!({"before":before.map(Self::state).transpose()?,"after":if before.is_none(){serde_json::to_value(task).map_err(|_|PlatformError::Storage)?}else{Self::state(task)?},"reason":reason}),
        )
    }
    pub fn state(task: &WorkTask) -> Result<serde_json::Value, PlatformError> {
        let mut value = serde_json::to_value(task).map_err(|_| PlatformError::Storage)?;
        value
            .as_object_mut()
            .ok_or(PlatformError::Storage)?
            .remove("template_snapshot");
        Ok(value)
    }
    pub fn event(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        task: &WorkTask,
        operation: &str,
        details: serde_json::Value,
    ) -> Result<(), PlatformError> {
        let sequence:i64=tx.query_one("UPDATE tasks SET history_sequence=history_sequence+1 WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 RETURNING history_sequence",&[&task.id,&task.project_id]).map_err(TaskWorkflowStore::database_error)?.get(0);
        let id = SecretGenerator::id("the")?;
        tx.execute("INSERT INTO task_history(id,project_id,task_id,sequence,revision,operation,actor_id,credential_id,details) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",&[&id,&task.project_id,&task.id,&sequence,&task.revision,&operation,&ctx.actor_id(),&ctx.credential_id(),&details]).map_err(TaskWorkflowStore::database_error)?;
        AuditWriter::append(
            tx,
            Some(ctx.actor_id()),
            Some(ctx.credential_id()),
            operation,
            Some(&task.id),
            json!({"project_id":task.project_id,"revision":task.revision,"sequence":sequence,"approval_id":details.get("approval_id"),"reason":details.get("reason")}),
        )
    }
}
