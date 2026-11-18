use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::{AuthContext, TaskTransitionRequest};
use serde_json::Value;

pub(crate) struct TaskTransitionExecutionContext<'a> {
    pub(crate) store: &'a mut TaskWorkflowStore,
    pub(crate) ctx: &'a AuthContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) task_id: &'a str,
    pub(crate) transition_id: &'a str,
    pub(crate) input: &'a TaskTransitionRequest,
    pub(crate) policy: &'a GitNetworkPolicy,
    pub(crate) run: &'a mut Value,
}
