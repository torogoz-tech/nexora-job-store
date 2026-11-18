/// Durable result after an unrecoverable task-knowledge failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKnowledgeFailure {
    pub run_id: String,
    pub transition_failed: bool,
}
