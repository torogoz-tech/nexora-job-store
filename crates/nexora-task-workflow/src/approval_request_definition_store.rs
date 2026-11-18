use crate::{
    SecretGenerator, TaskWorkflowStore, approval_operation_policy::ApprovalOperationPolicy,
    approval_request_history_writer::ApprovalRequestHistoryWriter,
    approval_request_query_store::ApprovalRequestQueryStore, receipt_writer::ReceiptWriter,
};
use nexora_platform::*;

pub(crate) struct ApprovalRequestDefinitionStore;
impl ApprovalRequestDefinitionStore {
    pub fn create(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: &str,
        input: &ApprovalRequestCreate,
    ) -> Result<ApprovalRequest, PlatformError> {
        if !(1..=86400).contains(&input.expires_in_seconds) {
            return Err(PlatformError::Validation(
                "invalid approval request duration",
            ));
        }
        let reason = DomainText::required(&input.reason, 4000)?;
        let request = store.request_context.clone();
        let namespace = store.namespace.clone();
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        ApprovalOperationPolicy::request_authority(&mut tx, ctx, project, &input.target)?;
        if let Some(value) = ReceiptWriter::read(
            &mut tx,
            &namespace,
            request.as_ref(),
            ctx.actor_id(),
            "approval_request.create",
        )? {
            tx.commit().map_err(TaskWorkflowStore::database_error)?;
            return TaskWorkflowStore::decode(value);
        }
        let snapshot = ApprovalOperationPolicy::snapshot(&mut tx, project, &input.target)?;
        let target = serde_json::to_value(&input.target).map_err(|_| PlatformError::Storage)?;
        let id = SecretGenerator::id("apr")?;
        let seconds = i64::from(input.expires_in_seconds);
        tx.execute("INSERT INTO approval_requests(id,project_id,target,target_kind,resource_id,version_id,transition_id,root_revision,resource_revision,workflow_version_id,from_state,to_state,operation_checksum,reason,requested_by,credential_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,clock_timestamp()+$17::bigint*interval '1 second')",&[&id,&project,&target,&snapshot.target_kind,&snapshot.resource_id,&snapshot.version_id,&snapshot.transition_id,&snapshot.root_revision,&snapshot.resource_revision,&snapshot.workflow_version_id,&snapshot.from_state,&snapshot.to_state,&snapshot.checksum,&reason,&ctx.actor_id(),&ctx.credential_id(),&seconds]).map_err(TaskWorkflowStore::database_error)?;
        let result = ApprovalRequestQueryStore::row(&mut tx, project, &id, false)?;
        ApprovalRequestHistoryWriter::append(
            &mut tx,
            ctx,
            &result,
            "approval_request.created",
            &reason,
        )?;
        ReceiptWriter::save(
            &mut tx,
            request.as_ref(),
            &serde_json::to_value(&result).map_err(|_| PlatformError::Storage)?,
        )?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(result)
    }
}
