use super::TaskTransitionEngine;
use crate::{
    GitNetworkPolicy, SecretGenerator, TaskWorkflowStore,
    task_transition_execution_context::TaskTransitionExecutionContext,
    task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionEngine {
    pub(crate) fn execute(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        policy: &GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        TaskTransitionRunStore::authorize(store, ctx, project, id)?;
        let namespace = store.namespace.clone();
        // Session advisory lock serializes execution without keeping a database transaction open during HTTP.
        let lock = format!("task-transition:{id}");
        let acquired: bool = store
            .client()
            .query_one(
                "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                &[&lock],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        if !acquired {
            return Err(PlatformError::Conflict(
                "task transition is already executing; inspect its run and retry",
            ));
        }
        let receipt_lock = store
            .request_context
            .as_ref()
            .map(|request| {
                serde_json::to_string(&(
                    &namespace,
                    ctx.actor_id(),
                    &request.operation,
                    &request.request_id,
                ))
                .map_err(|_| PlatformError::Storage)
            })
            .transpose()?;
        if let Some(receipt_lock) = &receipt_lock {
            store
                .client()
                .query_one(
                    "SELECT pg_advisory_lock(hashtextextended($1,0))",
                    &[receipt_lock],
                )
                .map_err(TaskWorkflowStore::database_error)?;
        }
        store.transition_verified_facts.clear();
        let executed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let request = store.request_context.clone();
            if let Some(request) = request.as_ref() {
                let mut tx = store
                    .client()
                    .transaction()
                    .map_err(TaskWorkflowStore::database_error)?;
                if let Some(value) = crate::receipt_writer::ReceiptWriter::read(
                    &mut tx,
                    &namespace,
                    Some(request),
                    ctx.actor_id(),
                    &request.operation,
                )? {
                    tx.commit().map_err(TaskWorkflowStore::database_error)?;
                    return TaskWorkflowStore::decode(value);
                }
                tx.commit().map_err(TaskWorkflowStore::database_error)?;
            }
            let result = Self::locked(store, ctx, project, id, transition, input, policy)?;
            if let Some(request) = request.as_ref() {
                let mut tx = store
                    .client()
                    .transaction()
                    .map_err(TaskWorkflowStore::database_error)?;
                crate::receipt_writer::ReceiptWriter::read(
                    &mut tx,
                    &namespace,
                    Some(request),
                    ctx.actor_id(),
                    &request.operation,
                )?;
                crate::receipt_writer::ReceiptWriter::save(&mut tx, Some(request), &json!(result))?;
                tx.commit().map_err(TaskWorkflowStore::database_error)?;
            }
            Ok(result)
        }));
        store.transition_verified_facts.clear();
        store.transition_run_checkpoint = None;
        if let Some(receipt_lock) = receipt_lock {
            store
                .client()
                .query_one(
                    "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                    &[&receipt_lock],
                )
                .map_err(TaskWorkflowStore::database_error)?;
        }
        store
            .client()
            .query_one(
                "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                &[&lock],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        match executed {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    pub(crate) fn locked(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        policy: &GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        let fingerprint = SecretGenerator::checksum(
            &serde_json::to_vec(
                &crate::task_approval_fingerprint::TaskApprovalFingerprint::parameters(input)?,
            )
            .map_err(|_| PlatformError::Storage)?,
        );
        let row=store.client().query_opt("SELECT record FROM task_transition_runs WHERE task_id=$1 AND task_revision=$2 AND transition_id=$3 AND actor_id=$4 AND fingerprint=$5",&[&id,&input.expected_revision,&transition,&ctx.actor_id(),&fingerprint]).map_err(TaskWorkflowStore::database_error)?;
        let mut run = if let Some(row) = row {
            row.get::<_, Value>(0)
        } else {
            let run = json!({"id":SecretGenerator::id("trn")?,"task_id":id,"actor_id":ctx.actor_id(),"request_id":store.request_context.as_ref().map(|request|&request.request_id),"request_operation":store.request_context.as_ref().map(|request|&request.operation),"transition_id":transition,"task_revision":input.expected_revision,"status":"running","next_step":0,"outcomes":[],"provider_intents":{},"advanced":false});
            store.client().execute("INSERT INTO task_transition_runs(id,task_id,project_id,actor_id,task_revision,transition_id,fingerprint,parameters,record,credential_id,auth_method) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",&[&run["id"].as_str(),&id,&project,&ctx.actor_id(),&input.expected_revision,&transition,&fingerprint,&json!(input),&run,&ctx.credential_id(),&if ctx.method()==AuthMethod::ApiKey{"api_key"}else{"session"}]).map_err(TaskWorkflowStore::database_error)?;
            run
        };
        if run["status"] == "succeeded" {
            let same_request = store.request_context.as_ref().is_some_and(|request| {
                run["request_id"].as_str() == Some(request.request_id.as_str())
                    && run["request_operation"].as_str() == Some(request.operation.as_str())
            });
            if same_request {
                return TaskWorkflowStore::decode(run["result"].clone());
            }
            return Err(PlatformError::Conflict(
                "task revision changed; inspect or retry the existing transition run",
            ));
        }
        // A newly granted approval may be attached without changing any approved operation parameters.
        store
            .client()
            .execute(
                "UPDATE task_transition_runs SET parameters=$2 WHERE id=$1",
                &[&run["id"].as_str(), &json!(input)],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        run["status"] = json!("running");
        run["error_code"] = Value::Null;
        run["message"] = Value::Null;
        TaskTransitionRunStore::save(store, &run)?;
        let mut execution = TaskTransitionExecutionContext {
            store,
            ctx,
            project,
            task_id: id,
            transition_id: transition,
            input,
            policy,
            run: &mut run,
        };
        let result = Self::resume(&mut execution);
        if let Err(ref error) = result {
            TaskTransitionRunStore::fail(store, ctx, &mut run, error)?;
            if run["status"] == "waiting_approval" {
                Self::request_approval(store, ctx, project, id, transition, input, &mut run)?;
            }
        }
        result
    }
}
