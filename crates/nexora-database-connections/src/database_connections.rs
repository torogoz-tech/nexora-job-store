use nexora_core::NexoraError;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConnections {
    pub schema_version: u16,
    pub host: String,
    pub postgres_port: u16,
    pub redis_port: u16,
    pub neo4j_http_port: u16,
    pub postgres_database: String,
    pub postgres_user: String,
    pub secrets_directory: PathBuf,
    #[serde(default)]
    pub allow_remote_host: bool,
}
impl DatabaseConnections {
    pub fn load(path: &Path) -> Result<Self, NexoraError> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(16_385)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16_384 {
            return Err(NexoraError::Configuration(
                "connection settings >16KiB".into(),
            ));
        }
        let mut settings: Self = serde_json::from_slice(&bytes)
            .map_err(|_| NexoraError::Configuration("invalid connection settings".into()))?;
        settings.validate()?;
        if settings.secrets_directory.is_relative() {
            settings.secrets_directory = path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&settings.secrets_directory);
        }
        Ok(settings)
    }
    pub fn validate(&self) -> Result<(), NexoraError> {
        if self.schema_version != 1
            || !Self::valid_host(&self.host, self.allow_remote_host)
            || self.postgres_port == 0
            || self.redis_port == 0
            || self.neo4j_http_port == 0
            || self.postgres_user.is_empty()
            || self.postgres_database.is_empty()
        {
            return Err(NexoraError::Configuration(
                "invalid database settings".into(),
            ));
        }
        Ok(())
    }

    fn valid_host(host: &str, allow_remote_host: bool) -> bool {
        if ["127.0.0.1", "localhost", "::1"].contains(&host) {
            return true;
        }

        if !allow_remote_host || host.is_empty() || host.len() > 253 {
            return false;
        }

        if host.parse::<IpAddr>().is_ok() {
            return true;
        }

        host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    }
}
