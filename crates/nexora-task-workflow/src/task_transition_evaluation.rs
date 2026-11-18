use crate::TaskWorkflowStore;
use nexora_platform::*;
use postgres::Transaction;
use serde_json::json;

pub(crate) struct TaskTransitionEvaluation;
impl TaskTransitionEvaluation {
    pub fn build(
        tx: &mut Transaction<'_>,
        before: &WorkTask,
        input: &TaskTransitionRequest,
        flow: &WorkflowVersion,
        transition: &str,
    ) -> Result<WorkflowEvaluationContext, PlatformError> {
        if input.expected_revision < 1 || (input.clear_resolution && input.resolution.is_some()) {
            return Err(PlatformError::Validation("invalid transition parameters"));
        }
        if let Some(reason) = &input.reason {
            DomainText::required(reason, 4000)?;
        }
        if let Some(resolution) = &input.resolution {
            DomainText::required(resolution, 4000)?;
        }
        let mut evaluation = WorkflowEvaluationContext {
            fields: before.fields.clone(),
            resolution: if input.clear_resolution {
                None
            } else {
                input
                    .resolution
                    .clone()
                    .or_else(|| before.resolution.clone())
            },
            reason: input.reason.clone(),
            ..Default::default()
        };
        evaluation
            .fields
            .insert("title".into(), json!(before.title));
        evaluation
            .fields
            .insert("context".into(), json!(before.context));
        evaluation
            .fields
            .insert("state_id".into(), json!(before.state_id));
        evaluation
            .fields
            .insert("priority".into(), json!(before.priority));
        evaluation
            .fields
            .insert("assignee_id".into(), json!(before.assignee_id));
        evaluation
            .fields
            .insert("resolution".into(), json!(evaluation.resolution));
        evaluation.fields.insert(
            "template_version_id".into(),
            json!(before.template_version_id),
        );
        if let Some(actor) = &before.assignee_id {
            evaluation.assignee_valid = tx
                .query_opt(
                    "SELECT id FROM actors WHERE id=$1 AND active FOR SHARE",
                    &[&actor],
                )
                .map_err(TaskWorkflowStore::database_error)?
                .is_some();
        }
        evaluation.dependencies_satisfied =
            crate::task_relation_policy::TaskRelationPolicy::blocking_count(tx, &before.id)? == 0;
        if flow.graph.transitions.iter().any(|edge| {
            edge.id == transition
                && before.state_id.as_deref() == Some(edge.from.as_str())
                && edge
                    .guards
                    .contains(&WorkflowGuard::ApprovedDocumentVersion)
        }) {
            evaluation.approved_document_version =
                crate::task_document_condition::TaskDocumentCondition::approved(tx, before)?;
        }
        Ok(evaluation)
    }
}
