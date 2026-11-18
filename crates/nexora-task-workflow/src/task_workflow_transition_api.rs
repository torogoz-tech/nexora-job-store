use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::{AuthContext, Capability, PlatformError, TaskTransitionRequest, WorkTask};

impl TaskWorkflowStore {
    /// Executes one configured task transition, including automatic final-stage dispatch.
    pub fn transition(
        &mut self,
        context: &AuthContext,
        project: Option<&str>,
        task_id: &str,
        transition_id: &str,
        input: &TaskTransitionRequest,
        policy: &GitNetworkPolicy,
    ) -> Result<WorkTask, PlatformError> {
        let actual_project = self.pool_task_scope(context, task_id, Capability::TasksTransition)?;
        if project.is_some() && project != actual_project.as_deref() {
            return Err(PlatformError::NotFound);
        }
        self.transition_pool_task_with_policy(context, task_id, transition_id, input, policy)
    }
}
