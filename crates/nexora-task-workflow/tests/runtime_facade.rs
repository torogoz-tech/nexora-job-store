use nexora_platform::{PlatformError, TaskTransitionRequest, WorkTask};
use nexora_task_workflow::{GitNetworkPolicy, TaskWorkflowStore};
use std::time::Duration;

type TransitionOperation = fn(
    &mut TaskWorkflowStore,
    &nexora_platform::AuthContext,
    Option<&str>,
    &str,
    &str,
    &TaskTransitionRequest,
    &GitNetworkPolicy,
) -> Result<WorkTask, PlatformError>;

#[test]
fn runtime_exposes_transition_and_recovery_operations_to_the_worker() {
    let _: TransitionOperation = TaskWorkflowStore::transition;
    let _: fn(&mut TaskWorkflowStore, &GitNetworkPolicy) -> Result<bool, PlatformError> =
        TaskWorkflowStore::resume_one_pending_transition;
    let _: fn(&mut TaskWorkflowStore) -> Result<Option<Duration>, PlatformError> =
        TaskWorkflowStore::next_waiting_external_delay;
}
