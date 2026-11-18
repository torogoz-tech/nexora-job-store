use nexora_platform::AuthMethod;

/// A durable transition run selected for continuation by a worker.
#[derive(Debug, Clone)]
pub(crate) struct PendingTaskTransition {
    pub(crate) id: String,
    pub(crate) task_id: String,
    pub(crate) actor_id: String,
    pub(crate) credential_id: String,
    pub(crate) auth_method: AuthMethod,
}
