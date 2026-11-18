use crate::{
    TaskWorkflowStore, task_history_writer::TaskHistoryWriter,
    task_scope_authority::TaskScopeAuthority,
};
use nexora_platform::*;
use serde_json::{Value, json};
pub(crate) struct TaskTransitionRunStore;
impl TaskTransitionRunStore {
    pub fn authorize(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
    ) -> Result<WorkTask, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        TaskScopeAuthority::require(&mut tx, ctx, Capability::TasksTransition, project)?;
        TaskScopeAuthority::modify(&mut tx, ctx, project, id, false)?;
        if project.is_none() {
            tx.query_opt("SELECT t.id FROM tasks t JOIN actor_roles r ON r.actor_id=$2 WHERE t.id=$1 AND t.assignee_id=$2 AND r.role IN ('developer','technical_lead','manager','pmo','super_admin') FOR SHARE OF r", &[&id,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        }
        let task = crate::task_policy_store::TaskPolicyStore::scoped_task(&mut tx, project, id)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(task)
    }
    pub fn save(store: &mut TaskWorkflowStore, run: &Value) -> Result<(), PlatformError> {
        store.client().execute("UPDATE task_transition_runs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",&[&run["id"].as_str().ok_or(PlatformError::Storage)?,run]).map_err(TaskWorkflowStore::database_error)?;
        Ok(())
    }
    pub fn fail(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        run: &mut Value,
        error: &PlatformError,
    ) -> Result<(), PlatformError> {
        let (code, message) = match error {
            PlatformError::Conflict("waiting for knowledge worker") => (
                "knowledge_worker_pending",
                "Waiting for asynchronous knowledge work to finish.",
            ),
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
            PlatformError::Conflict(message) => ("transition_conflict", *message),
            PlatformError::Validation(message) => ("transition_validation", *message),
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
        };
        let waiting_external = message.starts_with("waiting for ");
        run["status"] = json!(if code == "knowledge_worker_pending" {
            "waiting_knowledge"
        } else if waiting_external {
            "waiting_external"
        } else if code == "human_approval_required" {
            "waiting_approval"
        } else {
            "failed"
        });
        run["error_code"] = json!(if code == "knowledge_worker_pending" {
            "knowledge_worker_pending"
        } else if waiting_external {
            "provider_confirmation_pending"
        } else {
            code
        });
        run["message"] = json!(message);
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        // Recheck live authority before emitting any task-scoped failure information.
        let task = TaskScopeAuthority::task(
            &mut tx,
            ctx,
            run["task_id"].as_str().ok_or(PlatformError::Storage)?,
            Capability::TasksTransition,
        )?;
        tx.execute(
            "UPDATE task_transition_runs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
            &[&run["id"].as_str().ok_or(PlatformError::Storage)?, run],
        )
        .map_err(TaskWorkflowStore::database_error)?;
        TaskHistoryWriter::event(
            &mut tx,
            ctx,
            &task,
            if waiting_external {
                "task.transition_waiting_external"
            } else if code == "human_approval_required" {
                "task.transition_waiting_approval"
            } else {
                "task.transition_failed"
            },
            json!({"run_id":run["id"],"transition_id":run["transition_id"],"step_index":run["next_step"],"code":code,"message":message}),
        )?;
        tx.commit().map_err(TaskWorkflowStore::database_error)
    }
}
impl TaskWorkflowStore {
    pub fn task_transition_runs(
        &mut self,
        ctx: &AuthContext,
        id: &str,
    ) -> Result<Value, PlatformError> {
        let mut tx = self.client().transaction().map_err(Self::database_error)?;
        let task = TaskScopeAuthority::task(&mut tx, ctx, id, Capability::TasksRead)?;
        let items:Vec<Value>=tx.query("SELECT record||jsonb_build_object('parameters',parameters-'approval_id','notifications',COALESCE((SELECT jsonb_agg(jsonb_build_object('event_key',e.event_key,'status',e.status,'attempts',e.attempts)) FROM email_notifications e WHERE split_part(e.event_key,':',1)='workflow' AND split_part(e.event_key,':',2)=task_transition_runs.id),'[]'::jsonb)) FROM task_transition_runs WHERE task_id=$1 AND project_id IS NOT DISTINCT FROM $2 ORDER BY created_at DESC,id DESC LIMIT 100",&[&id,&task.project_id]).map_err(Self::database_error)?.into_iter().map(|row|row.get(0)).collect();
        tx.commit().map_err(Self::database_error)?;
        Ok(json!({"items":items}))
    }
    pub fn transition_pool_task_with_policy(
        &mut self,
        ctx: &AuthContext,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        policy: &crate::GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        let project = self.pool_task_scope(ctx, id, Capability::TasksTransition)?;
        let result = crate::task_transition_engine::TaskTransitionEngine::execute(
            self,
            ctx,
            project.as_deref(),
            id,
            transition,
            input,
            policy,
        )?;
        crate::task_transition_store::TaskTransitionStore::dispatch_final(
            &mut crate::final_transition_context::FinalTransitionContext {
                store: self,
                ctx,
                project: project.as_deref(),
                task_id: id,
                transition_id: transition,
                input,
                policy,
            },
            &result,
        )
    }
    pub fn transition_task_with_policy(
        &mut self,
        ctx: &AuthContext,
        project: &str,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        policy: &crate::GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        let result = crate::task_transition_engine::TaskTransitionEngine::execute(
            self,
            ctx,
            Some(project),
            id,
            transition,
            input,
            policy,
        )?;
        crate::task_transition_store::TaskTransitionStore::dispatch_final(
            &mut crate::final_transition_context::FinalTransitionContext {
                store: self,
                ctx,
                project: Some(project),
                task_id: id,
                transition_id: transition,
                input,
                policy,
            },
            &result,
        )
    }
    pub fn retry_task_transition(
        &mut self,
        ctx: &AuthContext,
        id: &str,
        run_id: &str,
        policy: &crate::GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        let project = self.pool_task_scope(ctx, id, Capability::TasksTransition)?;
        TaskTransitionRunStore::authorize(self, ctx, project.as_deref(), id)?;
        let row=self.client().query_opt("SELECT transition_id,parameters,record FROM task_transition_runs WHERE id=$1 AND task_id=$2 AND actor_id=$3",&[&run_id,&id,&ctx.actor_id()]).map_err(Self::database_error)?.ok_or(PlatformError::NotFound)?;
        let record: Value = row.get(2);
        if record["execution_kind"] == "take_branches" {
            return Err(PlatformError::Conflict(
                "retry task branch preparation through the take task operation",
            ));
        }
        let input: TaskTransitionRequest = Self::decode(row.get(1))?;
        let transition: String = row.get(0);
        if record["status"] == "succeeded" {
            let request = self.request_context.clone();
            let namespace = self.namespace.clone();
            if let Some(request) = request.as_ref() {
                let mut tx = self.client().transaction().map_err(Self::database_error)?;
                if let Some(value) = crate::receipt_writer::ReceiptWriter::read(
                    &mut tx,
                    &namespace,
                    Some(request),
                    ctx.actor_id(),
                    &request.operation,
                )? {
                    tx.commit().map_err(Self::database_error)?;
                    let result: WorkTask = Self::decode(value)?;
                    return crate::task_transition_store::TaskTransitionStore::dispatch_final(
                        &mut crate::final_transition_context::FinalTransitionContext {
                            store: self,
                            ctx,
                            project: project.as_deref(),
                            task_id: id,
                            transition_id: &transition,
                            input: &input,
                            policy,
                        },
                        &result,
                    );
                }
                crate::receipt_writer::ReceiptWriter::save(
                    &mut tx,
                    Some(request),
                    &record["result"],
                )?;
                tx.commit().map_err(Self::database_error)?;
            }
            let result: WorkTask = Self::decode(record["result"].clone())?;
            return crate::task_transition_store::TaskTransitionStore::dispatch_final(
                &mut crate::final_transition_context::FinalTransitionContext {
                    store: self,
                    ctx,
                    project: project.as_deref(),
                    task_id: id,
                    transition_id: &transition,
                    input: &input,
                    policy,
                },
                &result,
            );
        }
        let result = crate::task_transition_engine::TaskTransitionEngine::execute(
            self,
            ctx,
            project.as_deref(),
            id,
            &row.get::<_, String>(0),
            &input,
            policy,
        )?;
        crate::task_transition_store::TaskTransitionStore::dispatch_final(
            &mut crate::final_transition_context::FinalTransitionContext {
                store: self,
                ctx,
                project: project.as_deref(),
                task_id: id,
                transition_id: &row.get::<_, String>(0),
                input: &input,
                policy,
            },
            &result,
        )
    }
}
