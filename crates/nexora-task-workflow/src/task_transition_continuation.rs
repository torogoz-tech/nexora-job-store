use crate::{TaskWorkflowStore, task_transition_authority::TaskTransitionAuthority};
use nexora_platform::{AuthContext, AuthMethod, PlatformError};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub(crate) struct PendingTaskTransition {
    pub(crate) id: String,
    pub(crate) task_id: String,
    pub(crate) actor_id: String,
    pub(crate) credential_id: String,
    pub(crate) auth_method: AuthMethod,
    pub(crate) record: Value,
}

impl PendingTaskTransition {
    pub(crate) fn needs_provider_reconciliation(&self) -> bool {
        self.record["provider_intents"]
            .as_object()
            .is_some_and(|intents| {
                intents
                    .values()
                    .any(|intent| intent["status"] == "request_started")
            })
    }
}

/// Internal continuation-selection and original-authority failure operations.
pub(crate) struct TaskTransitionContinuation;

impl TaskTransitionContinuation {
    /// Selects one eligible run while acquiring the same session advisory lock as the engine.
    /// The caller must retain the store session through reconciliation and execution.
    pub(crate) fn select(
        store: &mut TaskWorkflowStore,
    ) -> Result<Option<PendingTaskTransition>, PlatformError> {
        let rows = store
            .client()
            .query(
                "SELECT id,task_id,actor_id,credential_id,auth_method,record,updated_at::text FROM task_transition_runs WHERE record->>'status'='ready' OR (record->>'status'='waiting_external' AND updated_at<clock_timestamp()-interval '30 seconds') OR (record->>'status'='running' AND updated_at<clock_timestamp()-interval '30 seconds') ORDER BY updated_at,id LIMIT 64",
                &[],
            )
            .map_err(|_| PlatformError::Storage)?;
        for row in rows {
            let task_id: String = row.get(1);
            let lock_name = format!("task-transition:{task_id}");
            let acquired: bool = store
                .client()
                .query_one(
                    "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                    &[&lock_name],
                )
                .map_err(|_| PlatformError::Storage)?
                .get(0);
            if !acquired {
                continue;
            }
            let run_id: String = row.get(0);
            let original_updated_at: String = row.get(6);
            let fresh = store.client().query_opt(
                "SELECT id,task_id,actor_id,credential_id,auth_method,record FROM task_transition_runs WHERE id=$1 AND task_id=$2 AND updated_at=$3::timestamptz AND (record->>'status'='ready' OR (record->>'status'='waiting_external' AND updated_at<clock_timestamp()-interval '30 seconds') OR (record->>'status'='running' AND updated_at<clock_timestamp()-interval '30 seconds'))",
                &[&run_id, &task_id, &original_updated_at],
            );
            let fresh = match fresh {
                Ok(Some(fresh)) => fresh,
                Ok(None) => {
                    Self::release_task_lock(store, &task_id)?;
                    continue;
                }
                Err(_) => {
                    Self::release_task_lock(store, &task_id)?;
                    return Err(PlatformError::Storage);
                }
            };
            let method = match fresh.get::<_, String>(4).as_str() {
                "session" => AuthMethod::Session,
                "api_key" => AuthMethod::ApiKey,
                _ => {
                    let _ = Self::release_task_lock(store, &task_id);
                    return Err(PlatformError::Storage);
                }
            };
            return Ok(Some(PendingTaskTransition {
                id: fresh.get(0),
                task_id,
                actor_id: fresh.get(2),
                credential_id: fresh.get(3),
                auth_method: method,
                record: fresh.get(5),
            }));
        }
        Ok(None)
    }

    fn revalidate_original_authority(
        store: &mut TaskWorkflowStore,
        pending: &PendingTaskTransition,
    ) -> Result<AuthContext, PlatformError> {
        let context = TaskTransitionAuthority::reload_credential(
            store,
            pending.auth_method,
            &pending.credential_id,
        )?;
        if !TaskTransitionAuthority::matches_recorded_actor(&context, &pending.actor_id) {
            return Err(PlatformError::Unauthorized);
        }
        Ok(context)
    }

    /// Returns current authority, or durably fails the run and releases its session lock.
    pub(crate) fn revalidate_or_fail(
        store: &mut TaskWorkflowStore,
        pending: &PendingTaskTransition,
    ) -> Result<Option<AuthContext>, PlatformError> {
        match Self::revalidate_original_authority(store, pending) {
            Ok(context) => Ok(Some(context)),
            Err(PlatformError::Storage) => {
                let _ = Self::release_task_lock(store, &pending.task_id);
                Err(PlatformError::Storage)
            }
            Err(_) => {
                let failed = Self::fail_authority_changed(store, pending, true);
                let released = Self::release_task_lock(store, &pending.task_id);
                failed?;
                released?;
                Ok(None)
            }
        }
    }

    /// Records live task-scope revocation using the existing terminal error and releases the lock.
    pub(crate) fn fail_scope_and_release(
        store: &mut TaskWorkflowStore,
        pending: &PendingTaskTransition,
    ) -> Result<(), PlatformError> {
        let failed = Self::fail_authority_changed(store, pending, false);
        let released = Self::release_task_lock(store, &pending.task_id);
        failed?;
        released?;
        Ok(())
    }

    pub(crate) fn fail_run_and_release(
        store: &mut TaskWorkflowStore,
        pending: &PendingTaskTransition,
        context: &AuthContext,
        run: &mut Value,
        error: &PlatformError,
    ) -> Result<(), PlatformError> {
        let failed =
            crate::task_transition_run::TaskTransitionRun::fail(store, context, run, error);
        let released = Self::release_task_lock(store, &pending.task_id);
        failed?;
        released?;
        Ok(())
    }

    fn fail_authority_changed(
        store: &mut TaskWorkflowStore,
        pending: &PendingTaskTransition,
        credential_invalid: bool,
    ) -> Result<(), PlatformError> {
        let patch = authority_failure_patch(credential_invalid);
        store
            .client()
            .execute(
                "UPDATE task_transition_runs SET record=record||$2::jsonb,updated_at=clock_timestamp() WHERE id=$1 AND record->>'status' IN ('ready','waiting_external','running')",
                &[&pending.id, &patch],
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(())
    }

    pub(crate) fn release_task_lock(
        store: &mut TaskWorkflowStore,
        task_id: &str,
    ) -> Result<bool, PlatformError> {
        store
            .client()
            .query_one(
                "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                &[&format!("task-transition:{task_id}")],
            )
            .map(|row| row.get(0))
            .map_err(|_| PlatformError::Storage)
    }
}

fn authority_failure_patch(credential_invalid: bool) -> Value {
    let message = if credential_invalid {
        "Original execution credential expired or was revoked. Restore authority and retry explicitly."
    } else {
        "Original execution authority changed. Reload the task and retry explicitly."
    };
    json!({
        "status": "failed",
        "error_code": "transition_authority_changed",
        "message": message
    })
}

#[cfg(test)]
mod tests {
    use super::PendingTaskTransition;
    use nexora_platform::AuthMethod;
    use serde_json::json;

    fn transition(record: serde_json::Value) -> PendingTaskTransition {
        PendingTaskTransition {
            id: "trn_1".into(),
            task_id: "tsk_1".into(),
            actor_id: "act_1".into(),
            credential_id: "ses_1".into(),
            auth_method: AuthMethod::Session,
            record,
        }
    }

    #[test]
    fn requires_reconciliation_before_replaying_started_provider_intent() {
        assert!(
            transition(json!({"provider_intents":{"pr:1":{"status":"request_started"}}}))
                .needs_provider_reconciliation()
        );
        assert!(
            !transition(json!({"provider_intents":{"pr:1":{"status":"confirmed"}}}))
                .needs_provider_reconciliation()
        );
        assert!(!transition(json!({"provider_intents":{}})).needs_provider_reconciliation());
    }

    #[test]
    fn missing_provider_intent_map_is_not_a_replay_barrier() {
        assert!(!transition(json!({"status":"running"})).needs_provider_reconciliation());
    }

    #[test]
    fn credential_failure_keeps_the_existing_terminal_fields_and_message() {
        assert_eq!(
            super::authority_failure_patch(true),
            json!({
                "status":"failed",
                "error_code":"transition_authority_changed",
                "message":"Original execution credential expired or was revoked. Restore authority and retry explicitly."
            })
        );
    }

    #[test]
    fn live_scope_failure_keeps_the_existing_terminal_fields_and_message() {
        assert_eq!(
            super::authority_failure_patch(false),
            json!({
                "status":"failed",
                "error_code":"transition_authority_changed",
                "message":"Original execution authority changed. Reload the task and retry explicitly."
            })
        );
    }
}
