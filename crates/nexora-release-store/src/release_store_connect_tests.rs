use crate::ReleaseStore;
use nexora_database_connections::DatabaseConnections;
use nexora_platform::PlatformError;
use std::path::PathBuf;

#[test]
fn rejects_unsafe_database_namespaces_before_connecting() {
    let settings = DatabaseConnections {
        schema_version: 1,
        host: "127.0.0.1".into(),
        postgres_port: 5432,
        redis_port: 6379,
        neo4j_http_port: 7474,
        postgres_database: "nexora".into(),
        postgres_user: "nexora".into(),
        secrets_directory: PathBuf::from("/nonexistent"),
        allow_remote_host: false,
    };

    assert!(matches!(
        ReleaseStore::connect(&settings, "nexora_platform;DROP SCHEMA public"),
        Err(PlatformError::Validation("invalid platform namespace"))
    ));
}
