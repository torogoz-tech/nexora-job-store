use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::AuthContext;
use serde_json::Value;

pub(crate) struct ProviderExecutionContext<'a> {
    pub(crate) store: &'a mut TaskWorkflowStore,
    pub(crate) ctx: &'a AuthContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) policy: &'a GitNetworkPolicy,
    pub(crate) run: &'a mut Value,
}
