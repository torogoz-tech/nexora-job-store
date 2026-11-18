use crate::{
    TaskWorkflowStore, task_transition_authority::TaskTransitionAuthority,
    task_transition_pending::PendingTaskTransition,
};
use nexora_platform::{AuthContext, AuthMethod, PlatformError};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

/// Internal continuation-selection and original-authority failure operations.
pub(crate) struct TaskTransitionContinuation;

impl TaskTransitionContinuation {
    /// Selects one eligible run while acquiring the same session advisory lock as the engine.
    /// The caller must retain the store session through reconciliation and execution.
    pub(crate) fn select(
        store: &mut TaskWorkflowStore,
    ) -> Result<Option<PendingTaskTransition>, PlatformError> {
        let scan_cutoff: SystemTime = store
            .client()
            .query_one("SELECT clock_timestamp()", &[])
            .map_err(|_| PlatformError::Storage)?
            .get(0);
        let waiting_external_cutoff = waiting_external_cutoff(scan_cutoff)?;
        let mut cursor: Option<(String, String)> = None;
        loop {
            let rows = match cursor.as_ref() {
                Some((updated_at, id)) => store.client().query(
                    "SELECT id,task_id,actor_id,credential_id,auth_method,record,updated_at::text FROM task_transition_runs WHERE (record->>'status'='ready' OR (record->>'status'='waiting_external' AND updated_at<$1) OR record->>'status'='running') AND (updated_at,id)>($2::timestamptz,$3::text) ORDER BY updated_at,id LIMIT 64",
                    &[&waiting_external_cutoff, updated_at, id],
                ),
                None => store.client().query(
                    "SELECT id,task_id,actor_id,credential_id,auth_method,record,updated_at::text FROM task_transition_runs WHERE record->>'status'='ready' OR (record->>'status'='waiting_external' AND updated_at<$1) OR record->>'status'='running' ORDER BY updated_at,id LIMIT 64",
                    &[&waiting_external_cutoff],
                ),
            }
            .map_err(|_| PlatformError::Storage)?;
            if rows.is_empty() {
                return Ok(None);
            }
            for row in rows {
                cursor = Some((row.get(6), row.get(0)));
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
                    "SELECT id,task_id,actor_id,credential_id,auth_method,record FROM task_transition_runs WHERE id=$1 AND task_id=$2 AND updated_at=$3::timestamptz AND (record->>'status'='ready' OR (record->>'status'='waiting_external' AND updated_at<$4) OR record->>'status'='running')",
                    &[&run_id, &task_id, &original_updated_at, &waiting_external_cutoff],
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
        }
    }

    /// Returns the DB-derived delay until the earliest waiting-external run is eligible.
    /// The worker can use it for one delayed wake instead of periodic polling.
    pub(crate) fn next_waiting_external_delay(
        store: &mut TaskWorkflowStore,
    ) -> Result<Option<Duration>, PlatformError> {
        let row = store
            .client()
            .query_opt(
                "SELECT EXTRACT(EPOCH FROM (MIN(updated_at+interval '30 seconds')-clock_timestamp()))::double precision FROM task_transition_runs WHERE record->>'status'='waiting_external' HAVING MIN(updated_at+interval '30 seconds')>clock_timestamp()",
                &[],
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(row.map(|row| delay_from_database_seconds(row.get(0))))
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

fn waiting_external_cutoff(scan_cutoff: SystemTime) -> Result<SystemTime, PlatformError> {
    scan_cutoff
        .checked_sub(Duration::from_secs(30))
        .ok_or(PlatformError::Storage)
}

fn delay_from_database_seconds(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds.max(0.0))
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
    use super::TaskTransitionContinuation;
    use serde_json::json;

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

    #[test]
    fn waiting_external_eligibility_uses_one_fixed_database_cutoff() {
        use std::time::{Duration, UNIX_EPOCH};

        let cutoff = UNIX_EPOCH + Duration::from_secs(1_000);
        let stale_before = super::waiting_external_cutoff(cutoff).unwrap();

        assert_eq!(stale_before, UNIX_EPOCH + Duration::from_secs(970));
        assert_eq!(
            super::waiting_external_cutoff(cutoff).unwrap(),
            stale_before
        );
    }

    #[test]
    fn future_waiting_external_recheck_delay_preserves_fractional_seconds() {
        assert_eq!(
            super::delay_from_database_seconds(2.5),
            std::time::Duration::from_millis(2_500)
        );
    }

    #[test]
    #[ignore = "requires isolated PostgreSQL with API migration 0088 applied"]
    fn selection_pages_past_sixty_four_candidates_locked_by_another_pod() {
        use crate::TaskWorkflowStore;
        use nexora_database_connections::{DatabaseConnections, PostgresConnection};
        use std::path::Path;

        let settings_path = std::env::var("NEXORA_TASK_WORKFLOW_TEST_DATABASE")
            .expect("set NEXORA_TASK_WORKFLOW_TEST_DATABASE to a private connection-settings file");
        let settings = DatabaseConnections::load(Path::new(&settings_path)).unwrap();
        let namespace = std::env::var("NEXORA_TASK_WORKFLOW_TEST_NAMESPACE")
            .unwrap_or_else(|_| "nexora_platform_test".into());
        let mut worker = TaskWorkflowStore::connect(&settings, &namespace).unwrap();
        let mut blocker = PostgresConnection::connect(&settings).unwrap();
        worker
            .client()
            .batch_execute(
                "CREATE TEMP TABLE task_transition_runs (
                    id text PRIMARY KEY,
                    task_id text NOT NULL,
                    actor_id text NOT NULL,
                    credential_id text NOT NULL,
                    auth_method text NOT NULL,
                    record jsonb NOT NULL,
                    updated_at timestamptz NOT NULL
                ) ON COMMIT PRESERVE ROWS",
            )
            .unwrap();

        let busy_lock = "task-transition:task-workflow-page-busy";
        assert!(
            blocker
                .client()
                .query_one(
                    "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                    &[&busy_lock],
                )
                .unwrap()
                .get::<_, bool>(0)
        );
        for index in 0..65_i32 {
            let id = format!("trn_page_{index:03}");
            let task_id = if index < 64 {
                "task-workflow-page-busy"
            } else {
                "task-workflow-page-available"
            };
            worker
                .client()
                .execute(
                    "INSERT INTO task_transition_runs(id,task_id,actor_id,credential_id,auth_method,record,updated_at) VALUES($1,$2,'act_test','ses_test','session','{\"status\":\"ready\"}',clock_timestamp()+($3::integer*interval '1 second'))",
                    &[&id, &task_id, &index],
                )
                .unwrap();
        }

        let pending = TaskTransitionContinuation::select(&mut worker)
            .unwrap()
            .expect("selection should scan beyond the locked first page");
        assert_eq!(pending.id, "trn_page_064");
        TaskTransitionContinuation::release_task_lock(&mut worker, &pending.task_id).unwrap();
        worker
            .client()
            .execute(
                "INSERT INTO task_transition_runs(id,task_id,actor_id,credential_id,auth_method,record,updated_at) VALUES('trn_waiting_due','task-workflow-waiting-due','act_test','ses_test','session','{\"status\":\"waiting_external\"}',clock_timestamp()-interval '27 seconds')",
                &[],
            )
            .unwrap();
        let delay = TaskTransitionContinuation::next_waiting_external_delay(&mut worker)
            .unwrap()
            .expect("future waiting-external recheck should be returned");
        assert!(delay.as_secs_f64() > 1.0 && delay.as_secs_f64() < 4.0);
        blocker
            .client()
            .query_one(
                "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                &[&busy_lock],
            )
            .unwrap();
    }
}
