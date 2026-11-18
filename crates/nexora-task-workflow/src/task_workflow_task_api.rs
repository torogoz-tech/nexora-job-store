use crate::{TaskWorkflowStore, task_transition_scope::TaskTransitionScope};
use nexora_platform::{AuthContext, Capability, PlatformError, WorkTask};

impl TaskWorkflowStore {
    pub fn pool_task_scope(
        &mut self,
        context: &AuthContext,
        id: &str,
        capability: Capability,
    ) -> Result<Option<String>, PlatformError> {
        let task = TaskTransitionScope::task(self, context, id, capability)?;
        Ok(task.project_id)
    }

    pub fn pool_task(
        &mut self,
        context: &AuthContext,
        id: &str,
    ) -> Result<WorkTask, PlatformError> {
        TaskTransitionScope::task(self, context, id, Capability::TasksRead)
    }
}
