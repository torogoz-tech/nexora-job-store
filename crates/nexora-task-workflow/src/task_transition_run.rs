use crate::TaskWorkflowStore;
use crate::{
    task_transition_audit::TaskTransitionAudit, task_transition_scope::TaskTransitionScope,
};
use nexora_platform::{AuthContext, Capability, PlatformError, WorkTask};
use serde_json::{Value, json};

/// Durable transition-run writes shared by the API adapter and continuation runtime.
pub(crate) struct TaskTransitionRun;

impl TaskTransitionRun {
    pub(crate) fn authorize(
        store: &mut TaskWorkflowStore,
        context: &AuthContext,
        task_id: &str,
    ) -> Result<WorkTask, PlatformError> {
        let mut transaction = store
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        let task = TaskTransitionScope::task_in_transaction(
            &mut transaction,
            context,
            task_id,
            Capability::TasksTransition,
        )?;
        if let Some(project_id) = task.project_id.as_deref() {
            transaction
                .query_one(
                    "SELECT pg_advisory_xact_lock_shared(hashtextextended(current_schema() || ':task-dependencies:' || $1::text,0))",
                    &[&project_id],
                )
                .map_err(|_| PlatformError::Storage)?;
        }
        let locked_task: Value = transaction
            .query_opt(
                "SELECT to_jsonb(t) FROM tasks t WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 FOR UPDATE",
                &[&task_id, &task.project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let task: WorkTask =
            serde_json::from_value(locked_task).map_err(|_| PlatformError::Storage)?;
        if task.project_id.is_none() {
            transaction
                .query_opt(
                    "SELECT t.id FROM tasks t WHERE t.id=$1 AND t.project_id IS NULL AND (t.created_by=$2 OR t.assignee_id=$2 OR EXISTS(SELECT 1 FROM actor_roles r WHERE r.actor_id=$2 AND r.role IN ('technical_lead','pmo','manager','super_admin'))) FOR UPDATE OF t",
                    &[&task_id, &context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Forbidden)?;
            Self::require_projectless_assignment(&mut transaction, context, task_id)?;
        }
        transaction.commit().map_err(|_| PlatformError::Storage)?;
        Ok(task)
    }

    pub(crate) fn save(
        store: &mut TaskWorkflowStore,
        run_id: &str,
        run: &Value,
    ) -> Result<(), PlatformError> {
        store
            .client()
            .execute(
                "UPDATE task_transition_runs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
                &[&run_id, run],
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(())
    }

    pub(crate) fn checkpoint_advanced(
        store: &mut TaskWorkflowStore,
        run_id: &str,
        result: &Value,
        next_step: i64,
    ) -> Result<(), PlatformError> {
        store
            .client()
            .execute(
                "UPDATE task_transition_runs SET record=record||jsonb_build_object('advanced',true,'result',$2::jsonb,'next_step',$3::bigint),updated_at=clock_timestamp() WHERE id=$1",
                &[&run_id, result, &next_step],
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(())
    }

    pub(crate) fn fail(
        store: &mut TaskWorkflowStore,
        context: &AuthContext,
        run: &mut Value,
        error: &PlatformError,
    ) -> Result<(), PlatformError> {
        let (code, message) = failure_details(error);
        let waiting_external = message.starts_with("waiting for ");
        run["status"] = json!(if waiting_external {
            "waiting_external"
        } else if code == "human_approval_required" {
            "waiting_approval"
        } else {
            "failed"
        });
        run["error_code"] = json!(if waiting_external {
            "provider_confirmation_pending"
        } else {
            code
        });
        run["message"] = json!(message);
        let mut transaction = store
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        let task_id = run["task_id"].as_str().ok_or(PlatformError::Storage)?;
        let task = TaskTransitionScope::task_in_transaction(
            &mut transaction,
            context,
            task_id,
            Capability::TasksTransition,
        )?;
        if task.project_id.is_none() {
            Self::require_projectless_assignment(&mut transaction, context, task_id)?;
        }
        transaction
            .execute(
                "UPDATE task_transition_runs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
                &[&run["id"].as_str().ok_or(PlatformError::Storage)?, run],
            )
            .map_err(|_| PlatformError::Storage)?;
        let operation = if waiting_external {
            "task.transition_waiting_external"
        } else if code == "human_approval_required" {
            "task.transition_waiting_approval"
        } else {
            "task.transition_failed"
        };
        let details = json!({
            "run_id": run["id"],
            "transition_id": run["transition_id"],
            "step_index": run["next_step"],
            "code": code,
            "message": message,
        });
        TaskTransitionAudit::event(&mut transaction, context, &task, operation, details)?;
        transaction.commit().map_err(|_| PlatformError::Storage)
    }

    fn require_projectless_assignment(
        transaction: &mut postgres::Transaction<'_>,
        context: &AuthContext,
        task_id: &str,
    ) -> Result<(), PlatformError> {
        transaction
            .query_opt(
                "SELECT t.id FROM tasks t JOIN actor_roles r ON r.actor_id=$2 WHERE t.id=$1 AND t.project_id IS NULL AND t.assignee_id=$2 AND r.role IN ('developer','technical_lead','manager','pmo','super_admin') FOR SHARE OF t,r",
                &[&task_id, &context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Forbidden)?;
        Ok(())
    }
}

fn failure_details(error: &PlatformError) -> (&'static str, &'static str) {
    match error {
        PlatformError::Conflict("human approval is required") => (
            "human_approval_required",
            "A human must approve these exact transition parameters before execution can continue.",
        ),
        PlatformError::Conflict(
            "task revision changed or task archived"
            | "task revision changed"
            | "task revision changed before provider write",
        ) => (
            "task_revision_changed",
            "The task changed. Reload it and start a new transition with its current revision.",
        ),
        PlatformError::Conflict(
            "provider branch head changed; refresh branch facts and obtain a new approval"
            | "provider branch head changed before provider write"
            | "provider branch head changed before pull request creation"
            | "pull request head or branches changed; refresh facts and approval",
        ) => (
            "provider_head_changed",
            "The provider branch or pull request changed. Refresh branch facts and obtain a new approval.",
        ),
        PlatformError::Conflict(
            "previous pull request outcome remains unknown; reconcile it at the provider before retrying",
        ) => (
            "provider_outcome_unknown",
            "The previous pull request outcome is uncertain. Reconcile it at the provider; Nexora will not create a duplicate.",
        ),
        PlatformError::Conflict(message) => ("transition_conflict", message),
        PlatformError::Validation(message) => ("transition_validation", message),
        PlatformError::NotFound => (
            "provider_resource_missing",
            "Required repository or branch is missing at the provider. Check the authorized repository and configured base branch, then retry.",
        ),
        PlatformError::Forbidden | PlatformError::Unauthorized => (
            "transition_authority_changed",
            "Access changed. Reconnect the Git provider or ask a project administrator to restore permission.",
        ),
        PlatformError::Unavailable => (
            "provider_outcome_unknown",
            "Provider request failed or outcome is uncertain. Retry to reconcile; another pull request will not be created blindly.",
        ),
        _ => (
            "transition_storage_failure",
            "Execution could not be persisted. Retry the same operation to reconcile its outcome.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskTransitionRun, failure_details};
    use nexora_platform::{ActorKind, AuthContext, AuthMethod, PlatformError};

    #[test]
    fn approval_failure_preserves_waiting_semantics() {
        assert_eq!(
            failure_details(&PlatformError::Conflict("human approval is required")),
            (
                "human_approval_required",
                "A human must approve these exact transition parameters before execution can continue."
            )
        );
    }

    #[test]
    fn ambiguous_provider_failure_preserves_no_duplicate_guidance() {
        assert_eq!(
            failure_details(&PlatformError::Unavailable),
            (
                "provider_outcome_unknown",
                "Provider request failed or outcome is uncertain. Retry to reconcile; another pull request will not be created blindly."
            )
        );
    }

    #[test]
    #[ignore = "requires isolated PostgreSQL with API migration 0088 applied"]
    fn projectless_failure_assignment_check_rejects_reassigned_actor() {
        use crate::TaskWorkflowStore;
        use nexora_database_connections::DatabaseConnections;
        use std::path::Path;

        let settings_path = std::env::var("NEXORA_TASK_WORKFLOW_TEST_DATABASE")
            .expect("set NEXORA_TASK_WORKFLOW_TEST_DATABASE to a private connection-settings file");
        let settings = DatabaseConnections::load(Path::new(&settings_path)).unwrap();
        let namespace = std::env::var("NEXORA_TASK_WORKFLOW_TEST_NAMESPACE")
            .unwrap_or_else(|_| "nexora_platform_test".into());
        let mut store = TaskWorkflowStore::connect(&settings, &namespace).unwrap();
        store
            .client()
            .batch_execute(
                "CREATE TEMP TABLE tasks (id text PRIMARY KEY, project_id text, assignee_id text);
                 CREATE TEMP TABLE actor_roles (actor_id text, role text);
                 INSERT INTO tasks VALUES ('tsk_projectless_fixture', NULL, 'act_projectless_fixture');
                 INSERT INTO actor_roles VALUES ('act_projectless_fixture', 'developer');",
            )
            .unwrap();
        let context = AuthContext::new(
            "act_projectless_fixture".into(),
            ActorKind::Human,
            AuthMethod::Session,
            "ses_projectless_fixture".into(),
            Vec::new(),
            None,
        );
        let mut transaction = store.client().transaction().unwrap();

        TaskTransitionRun::require_projectless_assignment(
            &mut transaction,
            &context,
            "tsk_projectless_fixture",
        )
        .unwrap();
        transaction
            .execute(
                "UPDATE tasks SET assignee_id='act_reassigned' WHERE id='tsk_projectless_fixture'",
                &[],
            )
            .unwrap();
        assert!(matches!(
            TaskTransitionRun::require_projectless_assignment(
                &mut transaction,
                &context,
                "tsk_projectless_fixture",
            ),
            Err(PlatformError::Forbidden)
        ));
    }
}
