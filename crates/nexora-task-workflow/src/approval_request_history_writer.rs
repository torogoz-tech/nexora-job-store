use crate::{task_history_writer::TaskHistoryWriter, task_policy_store::TaskPolicyStore};
use nexora_platform::{ApprovalRequest, ApprovalRequestTarget, AuthContext, PlatformError};
use postgres::Transaction;
use serde_json::json;

pub(crate) struct ApprovalRequestHistoryWriter;

impl ApprovalRequestHistoryWriter {
    pub(crate) fn append(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        request: &ApprovalRequest,
        event: &str,
        reason: &str,
    ) -> Result<(), PlatformError> {
        let ApprovalRequestTarget::Task { task_id, .. } = &request.target else {
            return Err(PlatformError::Validation(
                "task transitions can only write task approval history",
            ));
        };
        let task = TaskPolicyStore::task(tx, &request.project_id, task_id)?;
        TaskHistoryWriter::event(
            tx,
            ctx,
            &task,
            event,
            json!({"request_id":request.id,"approval_id":request.approval_id,"operation_checksum":request.operation_checksum,"status":request.status,"reason":reason}),
        )
    }
}
