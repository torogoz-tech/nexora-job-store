/// Claimed task-knowledge work for the knowledge worker.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskKnowledgeJob {
    pub job_id: String,
    pub job_kind: String,
    pub run_id: String,
    pub task_id: String,
    pub input: serde_json::Value,
    /// Per-claim fencing token. Completion must present the token returned by this claim.
    pub claim_token: String,
}
