use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_platform::PlatformError;
use postgres::{Client, error::SqlState};
use std::path::PathBuf;

pub struct NotificationStore {
    pub(crate) connection: PostgresConnection,
    pub(crate) namespace: String,
    pub(crate) secrets_directory: PathBuf,
}

impl NotificationStore {
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

        let mut connection =
            PostgresConnection::connect(settings).map_err(|_| PlatformError::Storage)?;
        let options = format!(
            "SET search_path TO {namespace},nexora_platform,pg_catalog; SET TIME ZONE 'UTC'; SET statement_timeout='15s'; SET lock_timeout='10s'"
        );
        connection
            .client()
            .batch_execute(&options)
            .map_err(Self::database_error)?;

        Ok(Self {
            connection,
            namespace: namespace.to_owned(),
            secrets_directory: settings.secrets_directory.clone(),
        })
    }

    pub(crate) fn client(&mut self) -> &mut Client {
        self.connection.client()
    }

    pub(crate) fn database_error(error: postgres::Error) -> PlatformError {
        match error.code() {
            Some(code) if code == &SqlState::UNIQUE_VIOLATION => {
                PlatformError::Conflict("resource already exists")
            }
            Some(code)
                if code == &SqlState::FOREIGN_KEY_VIOLATION
                    || code == &SqlState::CHECK_VIOLATION =>
            {
                PlatformError::Validation("invalid resource relation")
            }
            _ => PlatformError::Storage,
        }
    }

    pub fn smtp_runtime_settings(
        &mut self,
    ) -> Result<Option<crate::NotificationDeliveryRuntime>, PlatformError> {
        crate::notification_store_smtp::NotificationStoreSmtp::runtime_settings(self)
    }
}
