use nexora_database_connections::PostgresConnection;

/// Holds the PostgreSQL session for task workflow operations.
pub struct TaskWorkflowStore {
    pub(crate) _connection: PostgresConnection,
}

impl TaskWorkflowStore {
    pub(crate) fn client(&mut self) -> &mut postgres::Client {
        self._connection.client()
    }
}
