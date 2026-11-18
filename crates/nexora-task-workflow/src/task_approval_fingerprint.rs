use crate::SecretGenerator;
use nexora_platform::{PlatformError, TaskTransitionRequest, WorkTask, WorkflowTransition};
use serde_json::{Value, json};

pub(crate) struct TaskApprovalFingerprint;
impl TaskApprovalFingerprint {
    pub fn parameters(input: &TaskTransitionRequest) -> Result<Value, PlatformError> {
        let mut parameters = input.clone();
        parameters.approval_id = None;
        serde_json::to_value(parameters).map_err(|_| PlatformError::Storage)
    }

    pub fn checksum(
        task: &WorkTask,
        transition: &WorkflowTransition,
        input: &TaskTransitionRequest,
    ) -> Result<String, PlatformError> {
        let value = json!({"project_id":task.project_id,"task_id":task.id,"revision":task.revision,"workflow_id":task.workflow_id,"workflow_version_id":task.workflow_version_id,"transition":transition,"parameters":Self::parameters(input)?});
        let bytes = serde_json::to_vec(&value).map_err(|_| PlatformError::Storage)?;
        Ok(SecretGenerator::checksum(&bytes))
    }
}
