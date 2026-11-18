use crate::task_workflow_request_context::TaskWorkflowRequestContext;
use nexora_database_connections::PostgresConnection;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;

/// Holds the PostgreSQL session for task workflow operations.
pub struct TaskWorkflowStore {
    pub(crate) _connection: PostgresConnection,
    pub(crate) namespace: String,
    pub(crate) request_context: Option<TaskWorkflowRequestContext>,
    pub(crate) transition_verified_facts: Vec<Value>,
    pub(crate) transition_run_checkpoint: Option<(String, usize)>,
    pub(crate) secret_directory: PathBuf,
}

impl TaskWorkflowStore {
    pub(crate) fn client(&mut self) -> &mut postgres::Client {
        self._connection.client()
    }

    pub(crate) fn database_error(error: postgres::Error) -> nexora_platform::PlatformError {
        match error.code() {
            Some(code) if code == &postgres::error::SqlState::UNIQUE_VIOLATION => {
                nexora_platform::PlatformError::Conflict("resource already exists")
            }
            Some(code)
                if code == &postgres::error::SqlState::FOREIGN_KEY_VIOLATION
                    || code == &postgres::error::SqlState::CHECK_VIOLATION =>
            {
                nexora_platform::PlatformError::Validation("invalid resource relation")
            }
            _ => nexora_platform::PlatformError::Storage,
        }
    }

    pub(crate) fn decode<T: DeserializeOwned>(
        value: Value,
    ) -> Result<T, nexora_platform::PlatformError> {
        serde_json::from_value(value).map_err(|_| nexora_platform::PlatformError::Storage)
    }
}
