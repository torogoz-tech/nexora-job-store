use crate::{
    GitNetworkPolicy, TaskWorkflowStore, task_transition_continuation::TaskTransitionContinuation,
    task_transition_engine::TaskTransitionEngine,
    task_transition_execution_context::TaskTransitionExecutionContext,
    task_transition_run_store::TaskTransitionRunStore, task_transition_scope::TaskTransitionScope,
    task_transition_store::TaskTransitionStore,
};
use nexora_platform::{Capability, PlatformError, TaskTransitionRequest};
use serde_json::Value;
use std::time::Duration;

impl TaskWorkflowStore {
    /// Resumes one durable transition selected under the task's advisory lock.
    pub fn resume_one_pending_transition(
        &mut self,
        policy: &GitNetworkPolicy,
    ) -> Result<bool, PlatformError> {
        let Some(pending) = TaskTransitionContinuation::select(self)? else {
            return Ok(false);
        };
        let Some(context) = TaskTransitionContinuation::revalidate_or_fail(self, &pending)? else {
            return Ok(true);
        };
        let task = match TaskTransitionScope::task(
            self,
            &context,
            &pending.task_id,
            Capability::TasksTransition,
        ) {
            Ok(task) => task,
            Err(PlatformError::Forbidden | PlatformError::NotFound) => {
                TaskTransitionContinuation::fail_scope_and_release(self, &pending)?;
                return Ok(true);
            }
            Err(error) => {
                let _ = TaskTransitionContinuation::release_task_lock(self, &pending.task_id);
                return Err(error);
            }
        };
        let result =
            self.resume_selected_transition(&pending, &context, task.project_id.as_deref(), policy);
        let released = TaskTransitionContinuation::release_task_lock(self, &pending.task_id);
        result?;
        released?;
        Ok(true)
    }

    /// Returns the delay until the earliest external continuation is eligible.
    pub fn next_waiting_external_delay(&mut self) -> Result<Option<Duration>, PlatformError> {
        TaskTransitionContinuation::next_waiting_external_delay(self)
    }

    fn resume_selected_transition(
        &mut self,
        pending: &crate::task_transition_pending::PendingTaskTransition,
        context: &nexora_platform::AuthContext,
        project: Option<&str>,
        policy: &GitNetworkPolicy,
    ) -> Result<(), PlatformError> {
        let row = self
            .client()
            .query_opt(
                "SELECT transition_id,parameters,record FROM task_transition_runs WHERE id=$1 AND task_id=$2 AND actor_id=$3 AND record->>'status' IN ('ready','waiting_external','running')",
                &[&pending.id, &pending.task_id, &pending.actor_id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?;
        let transition: String = row.get(0);
        let input: TaskTransitionRequest = TaskWorkflowStore::decode(row.get(1))?;
        let mut run: Value = row.get(2);
        TaskTransitionRunStore::authorize(self, context, project, &pending.task_id)?;

        let mut execution = TaskTransitionExecutionContext {
            store: self,
            ctx: context,
            project,
            task_id: &pending.task_id,
            transition_id: &transition,
            input: &input,
            policy,
            run: &mut run,
        };
        let resumed = TaskTransitionEngine::resume(&mut execution);
        match resumed {
            Ok(task) => {
                TaskTransitionStore::dispatch_final(
                    &mut crate::final_transition_context::FinalTransitionContext {
                        store: self,
                        ctx: context,
                        project,
                        task_id: &pending.task_id,
                        transition_id: &transition,
                        input: &input,
                        policy,
                    },
                    &task,
                )?;
                Ok(())
            }
            Err(error) => {
                TaskTransitionRunStore::fail(self, context, &mut run, &error)?;
                if run["status"] == "waiting_approval" {
                    TaskTransitionEngine::request_approval(
                        self,
                        context,
                        project,
                        &pending.task_id,
                        &transition,
                        &input,
                        &mut run,
                    )?;
                }
                Ok(())
            }
        }
    }
}
