use crate::{
    TaskWorkflowStore, final_transition_context::FinalTransitionContext,
    receipt_writer::ReceiptWriter, task_history_writer::TaskHistoryWriter,
    task_policy_store::TaskPolicyStore,
};
use nexora_platform::*;
use serde_json::Value;

pub(crate) struct TaskTransitionStore;
impl TaskTransitionStore {
    pub(crate) fn dispatch_final(
        execution: &mut FinalTransitionContext<'_>,
        result: &WorkTask,
    ) -> Result<WorkTask, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let id = execution.task_id;
        let transition = execution.transition_id;
        let input = execution.input;
        let policy = execution.policy;
        if !result.governed_stage || result.lifecycle != TaskLifecycle::Active {
            return Ok(result.clone());
        }
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let current = TaskPolicyStore::scoped_task(&mut tx, project, id)?;
        if current.revision != result.revision {
            tx.commit().map_err(TaskWorkflowStore::database_error)?;
            return Ok(current);
        }
        let flow = crate::task_pool_workflow::TaskPoolWorkflow::flow(
            &mut tx,
            project,
            result.execution_version()?,
            false,
        )?;
        let decomposition:bool=tx.query_one("SELECT EXISTS(SELECT 1 FROM task_subtasks s JOIN tasks child ON child.id=s.child_task_id JOIN tasks parent ON parent.id=s.parent_task_id WHERE s.child_task_id=$1 AND s.removed_at IS NULL AND (s.work_kind='decomposition' OR (s.work_kind='corrective' AND child.workflow_version_id=parent.workflow_version_id)))",&[&id]).map_err(TaskWorkflowStore::database_error)?.get(0);
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        if decomposition
            || flow
                .graph
                .transitions
                .iter()
                .any(|e| e.id == transition && e.automatic)
        {
            return Ok(result.clone());
        }
        let Some(final_edge) = flow
            .graph
            .transitions
            .iter()
            .find(|e| e.automatic && Some(e.from.as_str()) == result.state_id.as_deref())
        else {
            return Ok(result.clone());
        };
        let mut parameters = input.clone();
        parameters.expected_revision = result.revision;
        parameters.approval_id = None;
        parameters.resolution = None;
        parameters.clear_resolution = false;
        let outer = store.request_context.take();
        let delivered = crate::task_transition_engine::TaskTransitionEngine::execute(
            store,
            ctx,
            project,
            id,
            &final_edge.id,
            &parameters,
            policy,
        );
        store.request_context = outer;
        // The entry into Fin succeeded. A failed final step is durably recorded by the engine;
        // retain Fin and expose its run, rather than rolling back a completed stage.
        match delivered {
            Ok(task) => Ok(task),
            Err(_) => Ok(result.clone()),
        }
    }
    pub(crate) fn advance(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
    ) -> Result<WorkTask, PlatformError> {
        if input.expected_revision < 1 || (input.clear_resolution && input.resolution.is_some()) {
            return Err(PlatformError::Validation(
                "invalid transition revision or resolution",
            ));
        }
        let reason = input
            .reason
            .as_deref()
            .map(|v| DomainText::required(v, 4000))
            .transpose()?;
        let request = store.request_context.clone();
        let namespace = store.namespace.clone();
        let verified_facts = store.transition_verified_facts.clone();
        let run_checkpoint = store.transition_run_checkpoint.clone();
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        crate::task_relation_lock::TaskRelationLock::acquire(&mut tx)?;
        crate::task_scope_authority::TaskScopeAuthority::require(
            &mut tx,
            ctx,
            Capability::TasksTransition,
            project,
        )?;
        crate::task_scope_authority::TaskScopeAuthority::modify(&mut tx, ctx, project, id, false)?;
        if project.is_none() {
            tx.query_opt("SELECT t.id FROM tasks t JOIN actor_roles r ON r.actor_id=$2 WHERE t.id=$1 AND t.assignee_id=$2 AND r.role IN ('developer','technical_lead','manager','pmo','super_admin') FOR SHARE OF r",&[&id,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        }
        if let Some(value) = ReceiptWriter::read(
            &mut tx,
            &namespace,
            request.as_ref(),
            ctx.actor_id(),
            "task.transition",
        )? {
            tx.commit().map_err(TaskWorkflowStore::database_error)?;
            return TaskWorkflowStore::decode(value);
        }
        let before = TaskPolicyStore::scoped_task(&mut tx, project, id)?;
        for fact in &verified_facts {
            let repository = fact["repository_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let project = project.ok_or(PlatformError::Forbidden)?;
            let current =
                crate::task_transition_provider_authority::TaskTransitionProviderAuthority::lock(
                    &mut tx, ctx, project, repository,
                )?;
            if current != *fact {
                return Err(PlatformError::Conflict(
                    "repository authority or development policy changed after provider verification",
                ));
            }
        }
        if before.revision != input.expected_revision {
            return Err(PlatformError::Conflict("task revision changed"));
        }
        if matches!(
            before.lifecycle,
            TaskLifecycle::Pending | TaskLifecycle::Paused
        ) {
            return Err(PlatformError::Conflict(
                "task must be taken and resumed before transitioning",
            ));
        }
        if before.archived {
            return Err(PlatformError::Conflict("task is archived"));
        }
        let flow = crate::task_pool_workflow::TaskPoolWorkflow::flow(
            &mut tx,
            project,
            before.execution_version()?,
            false,
        )?;
        let mut evaluation = crate::task_transition_evaluation::TaskTransitionEvaluation::build(
            &mut tx, &before, input, &flow, transition,
        )?;
        if flow.graph.transitions.iter().any(|edge| {
            edge.id == transition
                && before.state_id.as_deref() == Some(edge.from.as_str())
                && edge.guards.contains(&WorkflowGuard::WorkingBranchReported)
        }) {
            evaluation.working_branch_reported = crate::task_work_store::TaskWorkStore::condition(
                &mut tx, ctx, &before, transition,
            )?;
        }
        evaluation.human_approval_valid = crate::task_approval_store::TaskApprovalStore::consume(
            &mut tx, ctx, &before, &flow, transition, input,
        )?;
        // Document facts come from locked persisted prerequisites. Approval consumption
        // above rolls back together with any failed guard or state update.
        let selected = flow
            .graph
            .transition(before.execution_state()?, transition, &evaluation)?;
        let terminal = flow
            .graph
            .states
            .iter()
            .find(|state| state.id == selected.to)
            .is_some_and(|state| state.terminal);
        crate::task_relation_policy::TaskRelationPolicy::state(&mut tx, &before, terminal)?;
        let automatic_final = selected.automatic && selected.from == selected.to && terminal;
        if automatic_final && (input.resolution.is_some() || input.clear_resolution) {
            return Err(PlatformError::Validation(
                "automatic final pipelines cannot change the completed stage resolution",
            ));
        }
        let result: WorkTask = if automatic_final {
            before.clone()
        } else {
            let value:Value=tx.query_one("UPDATE tasks AS t SET state_id=$3,resolution=$4,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1 AND project_id IS NOT DISTINCT FROM $2 RETURNING to_jsonb(t)",&[&id,&project,&selected.to,&evaluation.resolution]).map_err(TaskWorkflowStore::database_error)?.get(0);
            TaskWorkflowStore::decode(value)?
        };
        TaskHistoryWriter::append(
            &mut tx,
            ctx,
            &result,
            if automatic_final {
                "task.final_pipeline_started"
            } else {
                "task.transitioned"
            },
            Some(&before),
            reason.as_deref(),
        )?;
        let public = serde_json::to_value(&result).map_err(|_| PlatformError::Storage)?;
        ReceiptWriter::save(&mut tx, request.as_ref(), &public)?;
        if let Some((run_id, next)) = &run_checkpoint {
            tx.execute("UPDATE task_transition_runs SET record=record||jsonb_build_object('advanced',true,'result',$2::jsonb,'next_step',$3::bigint),updated_at=clock_timestamp() WHERE id=$1", &[run_id,&public,&(*next as i64)]).map_err(TaskWorkflowStore::database_error)?;
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(result)
    }
}
