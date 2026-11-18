use crate::DatabaseConnections;
use nexora_core::NexoraError;
use postgres::{Client, Config, NoTls};
use std::{
    fs::{File, symlink_metadata},
    io::Read,
    time::Duration,
};

pub struct PostgresConnection {
    client: Client,
}

impl PostgresConnection {
    pub fn connect(settings: &DatabaseConnections) -> Result<Self, NexoraError> {
        settings.validate()?;
        let password = Self::password(settings)?;
        let mut config = Config::new();
        config
            .host(&settings.host)
            .port(settings.postgres_port)
            .user(&settings.postgres_user)
            .dbname(&settings.postgres_database)
            .password(&password)
            .connect_timeout(Duration::from_secs(5));

        let client = config
            .connect(NoTls)
            .map_err(|_| NexoraError::Storage("PostgreSQL connection failed".into()))?;
        Ok(Self { client })
    }

    pub fn client(&mut self) -> &mut Client {
        &mut self.client
    }

    fn password(settings: &DatabaseConnections) -> Result<String, NexoraError> {
        let path = settings.secrets_directory.join("postgres_password");
        let metadata = symlink_metadata(&path)?;
        if !metadata.file_type().is_file() {
            return Err(NexoraError::Configuration(
                "PostgreSQL credential must be a regular file".into(),
            ));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(NexoraError::Configuration(
                    "PostgreSQL credential permissions are too broad".into(),
                ));
            }
        }

        let mut password = String::new();
        File::open(path)?
            .take(16_385)
            .read_to_string(&mut password)?;
        if password.len() > 16_384 {
            return Err(NexoraError::Configuration(
                "PostgreSQL credential exceeds the size limit".into(),
            ));
        }

        let password = password.trim().to_owned();
        if password.is_empty() || password.len() > 1024 || password.contains(['\r', '\n', '\0']) {
            return Err(NexoraError::Configuration(
                "invalid PostgreSQL credential".into(),
            ));
        }

        Ok(password)
    }
}

#[cfg(test)]
mod tests {
    use super::PostgresConnection;
    use crate::DatabaseConnections;
    use std::path::PathBuf;

    fn settings(secrets_directory: PathBuf) -> DatabaseConnections {
        DatabaseConnections {
            schema_version: 1,
            host: "localhost".into(),
            postgres_port: 5432,
            redis_port: 6379,
            neo4j_http_port: 7474,
            postgres_database: "nexora".into(),
            postgres_user: "nexora".into(),
            secrets_directory,
            allow_remote_host: false,
        }
    }

    #[test]
    fn accepts_a_private_postgres_password_file() {
        let root = std::env::temp_dir().join(format!(
            "nexora-postgres-connection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("postgres_password");
        std::fs::write(&path, "test-password\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert_eq!(
            PostgresConnection::password(&settings(root.clone())).unwrap(),
            "test-password"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_group_or_world_readable_postgres_password_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "nexora-postgres-connection-insecure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("postgres_password");
        std::fs::write(&path, "test-password").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(PostgresConnection::password(&settings(root.clone())).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_symlinked_postgres_password_files() {
        let root = std::env::temp_dir().join(format!(
            "nexora-postgres-connection-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("secret");
        let link = root.join("postgres_password");
        std::fs::write(&target, "test-password").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link).unwrap();

        assert!(PostgresConnection::password(&settings(root.clone())).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
