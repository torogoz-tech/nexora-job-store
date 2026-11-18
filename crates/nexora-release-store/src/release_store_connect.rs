use crate::ReleaseStore;
use nexora_analysis_store::AnalysisStore;
use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_platform::PlatformError;

impl ReleaseStore {
    pub fn connect(settings: &DatabaseConnections, namespace: &str) -> Result<Self, PlatformError> {
        if namespace.is_empty()
            || namespace.len() > 63
            || !namespace.starts_with("nexora_platform")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PlatformError::Validation("invalid platform namespace"));
        }

        let mut client =
            PostgresConnection::connect(settings).map_err(|_| PlatformError::Storage)?;
        client
            .client()
            .batch_execute(&format!(
                "SET search_path TO {namespace},nexora_platform,pg_catalog; SET TIME ZONE 'UTC'; SET statement_timeout='15s'; SET lock_timeout='10s'"
            ))
            .map_err(ReleaseStore::database_error)?;
        let provider_store = AnalysisStore::connect(settings, namespace)?;

        Ok(Self {
            client,
            provider_store,
        })
    }
}
