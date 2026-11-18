use nexora_platform::AuthMethod;
use serde_json::Value;

/// A durable transition run selected for continuation by a worker.
#[derive(Debug, Clone)]
pub(crate) struct PendingTaskTransition {
    pub(crate) id: String,
    pub(crate) task_id: String,
    pub(crate) actor_id: String,
    pub(crate) credential_id: String,
    pub(crate) auth_method: AuthMethod,
    pub(crate) record: Value,
}

impl PendingTaskTransition {
    pub(crate) fn needs_provider_reconciliation(&self) -> bool {
        self.record["provider_intents"]
            .as_object()
            .is_some_and(|intents| {
                intents
                    .values()
                    .any(|intent| intent["status"] == "request_started")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::PendingTaskTransition;
    use nexora_platform::AuthMethod;
    use serde_json::json;

    fn transition(record: serde_json::Value) -> PendingTaskTransition {
        PendingTaskTransition {
            id: "trn_1".into(),
            task_id: "tsk_1".into(),
            actor_id: "act_1".into(),
            credential_id: "ses_1".into(),
            auth_method: AuthMethod::Session,
            record,
        }
    }

    #[test]
    fn requires_reconciliation_before_replaying_started_provider_intent() {
        assert!(
            transition(json!({"provider_intents":{"pr:1":{"status":"request_started"}}}))
                .needs_provider_reconciliation()
        );
        assert!(
            !transition(json!({"provider_intents":{"pr:1":{"status":"confirmed"}}}))
                .needs_provider_reconciliation()
        );
        assert!(!transition(json!({"provider_intents":{}})).needs_provider_reconciliation());
    }

    #[test]
    fn missing_provider_intent_map_is_not_a_replay_barrier() {
        assert!(!transition(json!({"status":"running"})).needs_provider_reconciliation());
    }
}
