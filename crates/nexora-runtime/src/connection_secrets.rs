use crate::DatabaseConnections;
use nexora_core::NexoraError;
use std::{
    fs::{File, symlink_metadata},
    io::Read,
    path::Path,
};
pub(crate) struct ConnectionSecrets {
    pub postgres: String,
    pub redis: String,
    pub neo4j_user: String,
    pub neo4j_password: String,
}
impl ConnectionSecrets {
    pub fn load(c: &DatabaseConnections) -> Result<Self, NexoraError> {
        let read = |name: &str| -> Result<String, NexoraError> {
            let mut data = String::new();
            let path = c.secrets_directory.join(name);
            let metadata = symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err(NexoraError::Configuration(
                    "secret path is not a regular file".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(NexoraError::Configuration(
                        "secret file permissions are too broad".into(),
                    ));
                }
            }
            read_secret(&path)?.take(16_385).read_to_string(&mut data)?;
            if data.len() > 16_384 {
                return Err(NexoraError::Configuration(
                    "secret file exceeds limit".into(),
                ));
            }
            Ok(data.trim().into())
        };
        let postgres = read("postgres_password")?;
        let flags = read("dragonfly_flags")?;
        let redis = flags
            .lines()
            .find_map(|line| line.strip_prefix("--requirepass="))
            .ok_or_else(|| NexoraError::Configuration("Dragonfly credential missing".into()))?
            .into();
        let neo = read("neo4j_auth")?;
        let (user, password) = neo
            .split_once('/')
            .ok_or_else(|| NexoraError::Configuration("Neo4j credential format".into()))?;
        let result = Self {
            postgres,
            redis,
            neo4j_user: user.into(),
            neo4j_password: password.into(),
        };
        if [
            &result.postgres,
            &result.redis,
            &result.neo4j_user,
            &result.neo4j_password,
        ]
        .iter()
        .any(|s| s.is_empty() || s.len() > 1024 || s.contains(['\r', '\n']))
        {
            return Err(NexoraError::Configuration(
                "invalid database credential".into(),
            ));
        }
        Ok(result)
    }
}

fn read_secret(path: &Path) -> Result<File, std::io::Error> {
    File::open(path)
}
