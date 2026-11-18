/// Durable result of completing a task-knowledge job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKnowledgeCompletion {
    pub run_id: String,
    pub transition_ready: bool,
    /// Generation to acknowledge after the publisher confirms the corresponding wake.
    pub wake_generation: Option<i64>,
}
