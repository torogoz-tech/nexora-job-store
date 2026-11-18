use crate::{
    TaskWorkflowStore, task_history_writer::TaskHistoryWriter, task_policy_store::TaskPolicyStore,
    task_scope_authority::TaskScopeAuthority,
};
use nexora_platform::*;
use serde_json::{Value, json};
/// Delivery changes responsibility and stage, never the identity of a task.
pub(crate) struct TaskStageStore;
impl TaskStageStore {
    pub fn handoff(
        execution: &mut crate::task_transition_execution_context::TaskTransitionExecutionContext<
            '_,
        >,
        workflow: &str,
        index: usize,
    ) -> Result<WorkTask, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let id = execution.task_id;
        let run = &*execution.run;
        let outcomes = &run["outcomes"];
        let task: WorkTask = TaskWorkflowStore::decode(run["result"].clone())?;
        let revision = task.revision;
        let verified_facts = store.transition_verified_facts.clone();
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        crate::task_relation_lock::TaskRelationLock::acquire(&mut tx)?;
        let before = TaskScopeAuthority::task(&mut tx, ctx, id, Capability::TasksTransition)?;
        TaskScopeAuthority::modify(&mut tx, ctx, before.project_id.as_deref(), id, false)?;
        let before = TaskPolicyStore::scoped_task(&mut tx, before.project_id.as_deref(), id)?;
        if before.revision != revision
            || before.archived
            || before.lifecycle != TaskLifecycle::Active
        {
            return Err(PlatformError::Conflict(
                "task changed before stage delivery",
            ));
        }
        let source = crate::task_pool_workflow::TaskPoolWorkflow::flow(
            &mut tx,
            before.project_id.as_deref(),
            before.execution_version()?,
            false,
        )?;
        if !source
            .graph
            .states
            .iter()
            .any(|s| Some(s.id.as_str()) == before.state_id.as_deref() && s.terminal)
        {
            return Err(PlatformError::Conflict(
                "complete the current workflow stage before delivery",
            ));
        }
        let child=tx.query_opt("SELECT work_kind,parent_task_id FROM task_subtasks WHERE child_task_id=$1 AND removed_at IS NULL FOR SHARE",&[&id]).map_err(TaskWorkflowStore::database_error)?;
        let corrective = child
            .as_ref()
            .is_some_and(|r| r.get::<_, String>(0) == "corrective");
        if child
            .as_ref()
            .is_some_and(|r| r.get::<_, String>(0) == "decomposition")
        {
            return Err(PlatformError::Conflict(
                "decomposition subtasks cannot execute final deliveries",
            ));
        }
        let corrective_parent = if corrective {
            let parent_id: String = child.as_ref().ok_or(PlatformError::Storage)?.get(1);
            Some(TaskPolicyStore::scoped_task(
                &mut tx,
                before.project_id.as_deref(),
                &parent_id,
            )?)
        } else {
            None
        };
        let destination_version = corrective_parent
            .as_ref()
            .and_then(|parent| parent.workflow_version_id.as_deref());
        let destination:Value=tx.query_opt("SELECT to_jsonb(v) FROM workflows w JOIN workflow_versions v ON v.id=COALESCE($3,w.latest_version_id) AND v.workflow_id=w.id WHERE w.id=$1 AND NOT w.archived AND (w.project_id IS NULL OR w.project_id=$2) FOR SHARE OF w,v",&[&workflow,&before.project_id,&destination_version]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?.get(0);
        let destination: WorkflowVersion = TaskWorkflowStore::decode(destination)?;
        if !destination
            .resource_kinds
            .contains(&WorkflowResourceKind::Task)
        {
            return Err(PlatformError::Validation(
                "destination workflow must support tasks",
            ));
        }
        destination.graph.validate()?;
        if corrective {
            let parent = corrective_parent.as_ref().ok_or(PlatformError::Storage)?;
            if parent.workflow_id.as_deref() != Some(workflow)
                || parent.workflow_version_id.as_deref() != Some(destination.id.as_str())
            {
                return Err(PlatformError::Conflict(
                    "corrective delivery must return to the parent task's current pinned workflow",
                ));
            }
        }
        let initial = destination.graph.states.iter().find(|s| s.initial).ok_or(
            PlatformError::Validation("destination workflow requires an initial state"),
        )?;
        let base = initial
            .required_base_branch
            .as_deref()
            .ok_or(PlatformError::Conflict(
                "destination workflow requires an initial base branch before delivery",
            ))?;
        {
            crate::task_delivery_evidence::TaskDeliveryEvidence::verify(
                outcomes, base, corrective,
            )?;
            let repositories = tx
                .query(
                    "SELECT repository_id FROM task_repository_branches WHERE task_id=$1",
                    &[&id],
                )
                .map_err(TaskWorkflowStore::database_error)?;
            for row in repositories {
                let repository: String = row.get(0);
                let project = before
                    .project_id
                    .as_deref()
                    .ok_or(PlatformError::Forbidden)?;
                let verified = verified_facts
                    .iter()
                    .find(|fact| fact["repository_id"] == repository)
                    .ok_or(PlatformError::Conflict(
                        "repository must be verified again before stage delivery",
                    ))?;
                let current=crate::task_transition_provider_authority::TaskTransitionProviderAuthority::lock(&mut tx,ctx,project,&repository)?;
                if current != *verified {
                    return Err(PlatformError::Conflict(
                        "repository authority changed before stage delivery",
                    ));
                }
                let branch=tx.query_one("SELECT branch_name,head_sha FROM task_repository_branches WHERE task_id=$1 AND repository_id=$2 FOR SHARE",&[&id,&repository]).map_err(TaskWorkflowStore::database_error)?;
                let head: String = branch.get(1);
                if !outcomes
                    .as_array()
                    .ok_or(PlatformError::Storage)?
                    .iter()
                    .any(|step| {
                        (step["kind"] == "verify_pull_request_merged"
                            || step["kind"] == "merge_pull_request")
                            && step["result"]["pull_requests"]
                                .as_array()
                                .is_some_and(|merges| {
                                    merges.iter().any(|merge| {
                                        merge["repository_id"] == repository
                                            && merge["head_sha"] == head
                                            && merge["target_branch"] == base
                                    })
                                })
                    })
                {
                    return Err(PlatformError::Conflict(
                        "task branch changed before stage delivery",
                    ));
                }
                if !outcomes
                    .as_array()
                    .ok_or(PlatformError::Storage)?
                    .iter()
                    .any(|step| {
                        (step["kind"] == "verify_pull_request_merged"
                            || step["kind"] == "merge_pull_request")
                            && step["result"]["pull_requests"]
                                .as_array()
                                .is_some_and(|merges| {
                                    merges.iter().any(|merge| {
                                        merge["repository_id"] == repository
                                            && merge["target_branch"] == base
                                            && merge["provenance"] == "provider_verified"
                                    })
                                })
                    })
                {
                    return Err(PlatformError::Conflict(
                        "delivery requires a verified merge for every task repository",
                    ));
                }
            }
        }
        let value:Value=tx.query_one("UPDATE tasks AS t SET workflow_id=$2,workflow_version_id=$3,state_id=$4,entry_workflow_id=$2,assignee_id=NULL,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1 RETURNING to_jsonb(t)",&[&id,&destination.workflow_id,&destination.id,&initial.id]).map_err(TaskWorkflowStore::database_error)?.get(0);
        let after: WorkTask = TaskWorkflowStore::decode(value)?;
        TaskHistoryWriter::event(
            &mut tx,
            ctx,
            &after,
            "task.workflow_delivered",
            json!({"before":TaskHistoryWriter::state(&before)?,"after":TaskHistoryWriter::state(&after)?,"evidence":outcomes}),
        )?;
        let mut checkpoint = run.clone();
        checkpoint["result"] = json!(after);
        checkpoint["next_step"] = json!(index + 1);
        checkpoint["outcomes"].as_array_mut().ok_or(PlatformError::Storage)?.push(json!({"step_index":index,"kind":"handoff","result":{"workflow_id":workflow,"task_revision":after.revision}}));
        tx.execute(
            "UPDATE task_transition_runs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
            &[
                &run["id"].as_str().ok_or(PlatformError::Storage)?,
                &checkpoint,
            ],
        )
        .map_err(TaskWorkflowStore::database_error)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(after)
    }
}
