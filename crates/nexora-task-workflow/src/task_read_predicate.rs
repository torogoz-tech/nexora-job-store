/// Parameters are internal SQL placeholders; all request values remain bound parameters.
pub(crate) fn task_read_predicate(actor: &str, unrestricted: &str) -> String {
    format!(
        "({unrestricted} OR t.assignee_id={actor} OR t.created_by={actor} OR (t.assignee_id IS NULL AND (t.lifecycle='pending' OR (t.lifecycle='active' AND EXISTS(SELECT 1 FROM developer_workflows member_workflow WHERE member_workflow.actor_id={actor} AND member_workflow.workflow_id=t.workflow_id)))) OR EXISTS(SELECT 1 FROM task_history h WHERE h.task_id=t.id AND h.project_id=t.project_id AND h.actor_id={actor} AND h.operation IN ('task.created','task.changed','task.reassigned_workflow') AND h.details->'after'->>'assignee_id' IS NOT NULL AND (h.details->'before'->>'assignee_id') IS DISTINCT FROM (h.details->'after'->>'assignee_id')))"
    )
}
