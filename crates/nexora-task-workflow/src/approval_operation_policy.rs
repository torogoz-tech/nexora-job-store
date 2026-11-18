use crate::{
    approval_operation_snapshot::ApprovalOperationSnapshot, platform_authority::PlatformAuthority,
    task_approval_fingerprint::TaskApprovalFingerprint, task_policy_store::TaskPolicyStore,
    task_transition_evaluation::TaskTransitionEvaluation,
};
use nexora_platform::*;
use postgres::Transaction;

pub(crate) struct ApprovalOperationPolicy;

impl ApprovalOperationPolicy {
    pub(crate) fn request_authority(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        target: &ApprovalRequestTarget,
    ) -> Result<(), PlatformError> {
        if !matches!(target, ApprovalRequestTarget::Task { .. }) {
            return Err(PlatformError::Validation(
                "task transitions can only request task approval",
            ));
        }
        PlatformAuthority::require(tx, ctx, Capability::TasksRead, Some(project))?;
        PlatformAuthority::require(tx, ctx, Capability::TasksTransition, Some(project))?;
        PlatformAuthority::project(tx, project)
    }

    pub(crate) fn snapshot(
        tx: &mut Transaction<'_>,
        project: &str,
        target: &ApprovalRequestTarget,
    ) -> Result<ApprovalOperationSnapshot, PlatformError> {
        let ApprovalRequestTarget::Task {
            task_id,
            transition_id,
            parameters,
        } = target
        else {
            return Err(PlatformError::Validation(
                "task transitions can only request task approval",
            ));
        };
        if parameters.approval_id.is_some() {
            return Err(PlatformError::Validation(
                "approval request cannot contain a nested approval",
            ));
        }
        let task = TaskPolicyStore::task(tx, project, task_id)?;
        if task.archived || task.revision != parameters.expected_revision {
            return Err(PlatformError::Conflict(
                "requested task revision changed or archived",
            ));
        }
        let flow = TaskPolicyStore::flow(tx, project, task.execution_version()?, false)?;
        let mut context =
            TaskTransitionEvaluation::build(tx, &task, parameters, &flow, transition_id)?;
        context.human_approval_valid = true;
        let edge = flow
            .graph
            .transition(task.execution_state()?, transition_id, &context)?;
        if !edge.requires_approval() {
            return Err(PlatformError::Validation(
                "requested transition does not require human approval",
            ));
        }
        Ok(ApprovalOperationSnapshot {
            target_kind: "task",
            resource_id: task.id.clone(),
            version_id: None,
            transition_id: edge.id.clone(),
            root_revision: task.revision,
            resource_revision: task.revision,
            workflow_version_id: flow.id.clone(),
            from_state: edge.from.clone(),
            to_state: edge.to.clone(),
            checksum: TaskApprovalFingerprint::checksum(&task, edge, parameters)?,
        })
    }
}
