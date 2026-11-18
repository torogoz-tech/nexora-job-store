/// Idempotency context for one task workflow mutation.
#[derive(Clone)]
pub(crate) struct TaskWorkflowRequestContext {
    pub(crate) actor_id: String,
    pub(crate) operation: String,
    pub(crate) request_id: String,
    pub(crate) fingerprint: String,
}
