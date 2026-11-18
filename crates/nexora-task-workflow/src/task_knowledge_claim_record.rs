pub(crate) struct TaskKnowledgeClaimRecord {
    pub(crate) run_id: String,
    pub(crate) task_id: String,
    pub(crate) actor_id: String,
    pub(crate) retry_count: i32,
}
