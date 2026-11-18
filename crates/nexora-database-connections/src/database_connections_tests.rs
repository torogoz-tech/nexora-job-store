use crate::DatabaseConnections;
use std::path::PathBuf;

fn connections(allow_remote_host: bool, host: &str) -> DatabaseConnections {
    DatabaseConnections {
        schema_version: 1,
        host: host.into(),
        postgres_port: 5432,
        redis_port: 6379,
        neo4j_http_port: 7474,
        postgres_database: "nexora".into(),
        postgres_user: "nexora".into(),
        secrets_directory: PathBuf::from("secrets"),
        allow_remote_host,
    }
}

#[test]
fn remote_database_hosts_require_explicit_opt_in_and_valid_dns_or_ip() {
    assert!(connections(false, "localhost").validate().is_ok());
    assert!(connections(false, "db.internal").validate().is_err());
    assert!(connections(true, "db.internal").validate().is_ok());
    assert!(connections(true, "-invalid.internal").validate().is_err());
    assert!(connections(true, "127.0.0.1").validate().is_ok());
}

#[test]
fn settings_loader_rejects_unknown_fields_and_resolves_relative_secret_directory() {
    let root = std::env::temp_dir().join(format!(
        "nexora-database-connections-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("connections.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "schema_version": 1,
            "host": "localhost",
            "postgres_port": 5432,
            "redis_port": 6379,
            "neo4j_http_port": 7474,
            "postgres_database": "nexora",
            "postgres_user": "nexora",
            "secrets_directory": "secrets",
            "allow_remote_host": false
        })
        .to_string(),
    )
    .unwrap();

    let settings = DatabaseConnections::load(&path).unwrap();

    assert_eq!(settings.secrets_directory, root.join("secrets"));
    std::fs::write(
        &path,
        serde_json::json!({
            "schema_version": 1,
            "host": "localhost",
            "postgres_port": 5432,
            "redis_port": 6379,
            "neo4j_http_port": 7474,
            "postgres_database": "nexora",
            "postgres_user": "nexora",
            "secrets_directory": "secrets",
            "allow_remote_host": false,
            "unexpected": true
        })
        .to_string(),
    )
    .unwrap();

    assert!(DatabaseConnections::load(&path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
