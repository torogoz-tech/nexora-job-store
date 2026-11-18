/// Durable task-knowledge work that the API can translate into a RabbitMQ wakeup.
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKnowledgeWakeup {
    pub job_id: String,
    pub job_kind: String,
    pub status: String,
    pub run_id: String,
    pub task_id: String,
    /// PostgreSQL time after which the wakeup should become deliverable.
    pub available_at: SystemTime,
}
