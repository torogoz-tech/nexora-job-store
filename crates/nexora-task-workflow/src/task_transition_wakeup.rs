use crate::TaskWorkflowStore;
use nexora_platform::PlatformError;

const PENDING_WAKEUPS: &str = "SELECT id,task_id,wake_generation FROM task_transition_runs WHERE record->>'status'='ready' AND wake_pending ORDER BY updated_at,id";
const ACKNOWLEDGE_WAKEUP: &str = "UPDATE task_transition_runs SET wake_pending=false WHERE id=$1 AND wake_pending AND wake_generation=$2";

/// Durable identifiers for transition runs ready to resume in the release worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTransitionWakeup {
    pub run_id: String,
    pub task_id: String,
    pub generation: i64,
}

impl TaskWorkflowStore {
    /// Lists ready transition runs so callers can republish wakeups after startup/reconnect.
    pub fn pending_task_transition_wakeups(
        &mut self,
    ) -> Result<Vec<TaskTransitionWakeup>, PlatformError> {
        let rows = self
            .client()
            .query(PENDING_WAKEUPS, &[])
            .map_err(TaskWorkflowStore::database_error)?;

        Ok(rows
            .into_iter()
            .map(|row| TaskTransitionWakeup {
                run_id: row.get(0),
                task_id: row.get(1),
                generation: row.get(2),
            })
            .collect())
    }

    /// Clears only the generation confirmed by RabbitMQ; later wake intents remain pending.
    pub fn acknowledge_task_transition_wakeup(
        &mut self,
        run_id: &str,
        generation: i64,
    ) -> Result<bool, PlatformError> {
        let cleared = self
            .client()
            .execute(ACKNOWLEDGE_WAKEUP, &[&run_id, &generation])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(cleared == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{ACKNOWLEDGE_WAKEUP, PENDING_WAKEUPS};

    #[test]
    fn pending_scan_reads_only_ready_unacknowledged_generations() {
        assert!(PENDING_WAKEUPS.contains("status'='ready'"));
        assert!(PENDING_WAKEUPS.contains("AND wake_pending"));
        assert!(PENDING_WAKEUPS.contains("wake_generation"));
    }

    #[test]
    fn acknowledgement_is_fenced_to_the_published_generation() {
        assert!(ACKNOWLEDGE_WAKEUP.contains("wake_generation=$2"));
        assert!(ACKNOWLEDGE_WAKEUP.contains("wake_pending=false"));
    }
}
