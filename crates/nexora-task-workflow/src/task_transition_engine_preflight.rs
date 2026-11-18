use super::TaskTransitionEngine;
use crate::TaskWorkflowStore;
use nexora_platform::*;

impl TaskTransitionEngine {
    pub(crate) fn preflight(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        flow: &WorkflowVersion,
    ) -> Result<(), PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let task = crate::task_policy_store::TaskPolicyStore::scoped_task(&mut tx, project, id)?;
        if tx.query_one("SELECT EXISTS(SELECT 1 FROM task_transition_runs WHERE task_id=$1 AND task_revision=$2 AND record->>'execution_kind'='take_branches' AND record->>'status'<>'succeeded')",&[&id,&task.revision]).map_err(TaskWorkflowStore::database_error)?.get::<_,bool>(0){
            return Err(PlatformError::Conflict("repository work preparation is incomplete; retry Take with the same repository selection before transitioning this stage"));
        }
        if task.governed_stage && task.assignee_id.is_none() {
            return Err(PlatformError::Conflict(
                "take the current workflow stage before transitioning",
            ));
        }
        if task.archived || task.revision != input.expected_revision {
            return Err(PlatformError::Conflict(
                "task revision changed or task archived",
            ));
        }
        if matches!(
            task.lifecycle,
            TaskLifecycle::Pending | TaskLifecycle::Paused
        ) {
            return Err(PlatformError::Conflict(
                "task must be taken and resumed before transitioning",
            ));
        }
        let mut evaluation = crate::task_transition_evaluation::TaskTransitionEvaluation::build(
            &mut tx, &task, input, flow, transition,
        )?;
        evaluation.working_branch_reported =
            crate::task_work_store::TaskWorkStore::condition(&mut tx, ctx, &task, transition)?;
        let definition = flow
            .graph
            .transitions
            .iter()
            .find(|edge| edge.id == transition)
            .ok_or(PlatformError::Validation("transition is outside workflow"))?;
        evaluation.human_approval_valid =
            if definition.guards.contains(&WorkflowGuard::HumanApproval) {
                crate::task_approval_store::TaskApprovalStore::consume(
                    &mut tx, ctx, &task, flow, transition, input,
                )?
            } else {
                if !definition.requires_approval() && input.approval_id.is_some() {
                    return Err(PlatformError::Validation(
                        "transition does not accept an approval",
                    ));
                }
                // Ordered approval is evaluated only when its step is reached.
                true
            };
        let edge = flow
            .graph
            .transition(task.execution_state()?, transition, &evaluation)?;
        let terminal = flow
            .graph
            .states
            .iter()
            .any(|state| state.id == edge.to && state.terminal);
        crate::task_relation_policy::TaskRelationPolicy::state(&mut tx, &task, terminal)?;
        // Validation is read-only in effect: approval is consumed only with atomic advancement.
        tx.rollback().map_err(TaskWorkflowStore::database_error)
    }
}
