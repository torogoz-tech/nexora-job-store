use crate::TaskWorkflowStore;
use nexora_platform::{ApprovalRequest, PlatformError};
use postgres::Transaction;

pub(crate) struct ApprovalRequestQueryStore;

impl ApprovalRequestQueryStore {
    pub(crate) fn row(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
        lock: bool,
    ) -> Result<ApprovalRequest, PlatformError> {
        let sql = if lock {
            "SELECT to_jsonb(r)||jsonb_build_object('approval_id',task_approval_id) FROM approval_requests r WHERE id=$1 AND project_id=$2 AND target_kind='task' FOR UPDATE"
        } else {
            "SELECT to_jsonb(r)||jsonb_build_object('approval_id',task_approval_id) FROM approval_requests r WHERE id=$1 AND project_id=$2 AND target_kind='task'"
        };
        let value = tx
            .query_opt(sql, &[&id, &project])
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        TaskWorkflowStore::decode(value)
    }
}
