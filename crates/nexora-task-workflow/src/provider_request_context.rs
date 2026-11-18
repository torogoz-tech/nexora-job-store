use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::AuthContext;

pub(crate) struct ProviderRequestContext<'a> {
    pub(crate) store: &'a mut TaskWorkflowStore,
    pub(crate) ctx: &'a AuthContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) policy: &'a GitNetworkPolicy,
}
