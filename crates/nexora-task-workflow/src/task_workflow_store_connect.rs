use crate::TaskWorkflowStore;
use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_platform::PlatformError;

impl TaskWorkflowStore {
    /// Connects to an existing platform schema using the configured database credentials.
    pub fn connect(settings: &DatabaseConnections, namespace: &str) -> Result<Self, PlatformError> {
        if namespace.len() > 63
            || !namespace.starts_with("nexora_platform")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PlatformError::Validation("invalid platform namespace"));
        }

        settings
            .validate()
            .map_err(|_| PlatformError::Validation("invalid database configuration"))?;

        let mut connection =
            PostgresConnection::connect(settings).map_err(|_| PlatformError::Storage)?;
        connection
            .client()
            .batch_execute(&format!(
                "SET search_path TO {namespace},nexora_platform,pg_catalog;
                 SET TIME ZONE 'UTC';
                 SET statement_timeout = '15s';
                 SET lock_timeout = '10s';"
            ))
            .map_err(|_| PlatformError::Storage)?;

        Ok(Self {
            _connection: connection,
            namespace: namespace.to_owned(),
            request_context: None,
            transition_verified_facts: Vec::new(),
            transition_run_checkpoint: None,
            secret_directory: settings.secrets_directory.clone(),
        })
    }
}
