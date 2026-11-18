use std::time::SystemTime;

/// Durable result after a transient task-knowledge failure is rescheduled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKnowledgeRetry {
    pub run_id: String,
    pub task_id: String,
    pub retry_count: i32,
    pub available_at: SystemTime,
    pub terminal: bool,
    /// Whether this terminal job also made its transition terminal.
    pub transition_failed: bool,
}
