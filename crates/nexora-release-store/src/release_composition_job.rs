use nexora_platform::{AuthMethod, PlatformError};
use postgres::Row;
use serde_json::Value;

pub(crate) struct ReleaseCompositionJob {
    pub(crate) id: String,
    pub(crate) release_id: String,
    pub(crate) project_id: String,
    pub(crate) release_revision: i64,
    pub(crate) actor_id: String,
    pub(crate) credential_id: String,
    pub(crate) auth_method: AuthMethod,
    pub(crate) record: Value,
}

impl ReleaseCompositionJob {
    pub(crate) fn from_row(row: &Row) -> Result<Self, PlatformError> {
        Ok(Self {
            id: row.get(0),
            release_id: row.get(1),
            project_id: row.get(2),
            release_revision: row.get(3),
            actor_id: row.get(4),
            credential_id: row.get(5),
            auth_method: Self::auth_method(&row.get::<_, String>(6))?,
            record: row.get(7),
        })
    }

    pub(crate) fn auth_method(value: &str) -> Result<AuthMethod, PlatformError> {
        match value {
            "session" => Ok(AuthMethod::Session),
            "api_key" => Ok(AuthMethod::ApiKey),
            _ => Err(PlatformError::Storage),
        }
    }
}
