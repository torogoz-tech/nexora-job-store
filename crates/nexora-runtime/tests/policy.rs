use nexora_indexer::FileCacheKey;
use nexora_runtime::{CacheRetention, DragonflyCache};

#[test]
fn retention_is_bounded_and_identity_includes_all_analysis_inputs() {
    assert_eq!(CacheRetention::seconds(0, 0), 300);
    assert!(CacheRetention::seconds(u64::MAX, usize::MAX) <= 86_400);
    assert!(CacheRetention::seconds(1_000_000, 1024) >= CacheRetention::seconds(1_000, 1024));
    let key = FileCacheKey {
        tenant: "a".into(),
        repository: "r".into(),
        configuration: "c".into(),
        engine: "e".into(),
        path: "a.js".into(),
        content_hash: "h".into(),
    };
    let original = DragonflyCache::key(&key).unwrap();
    for field in [
        "tenant",
        "repository",
        "configuration",
        "engine",
        "path",
        "content_hash",
    ] {
        let mut value = serde_json::to_value(&key).unwrap();
        value[field] = serde_json::json!("different");
        let other: FileCacheKey = serde_json::from_value(value).unwrap();
        assert_ne!(original, DragonflyCache::key(&other).unwrap(), "{field}");
    }
    assert!(!original.contains("a.js"));
}

#[test]
fn malformed_configuration_is_rejected_without_reading_credentials() {
    use nexora_runtime::DatabaseConnections;
    let valid = serde_json::json!({"schema_version":1,"host":"127.0.0.1","postgres_port":15432,"redis_port":16379,"neo4j_http_port":17474,"postgres_user":"nexora","postgres_database":"nexora","secrets_directory":"missing-secrets"});
    let config: DatabaseConnections = serde_json::from_value(valid.clone()).unwrap();
    config.validate().unwrap();
    for field in [
        "schema_version",
        "postgres_port",
        "redis_port",
        "neo4j_http_port",
    ] {
        let mut value = valid.clone();
        value[field] = serde_json::json!(0);
        let config: DatabaseConnections = serde_json::from_value(value).unwrap();
        assert!(config.validate().is_err(), "{field}");
    }
    let mut unknown = valid;
    unknown["unrecognized"] = serde_json::json!(true);
    assert!(serde_json::from_value::<DatabaseConnections>(unknown).is_err());
}

#[test]
fn remote_database_hosts_require_explicit_opt_in_and_valid_dns_or_ip() {
    use nexora_runtime::DatabaseConnections;

    let mut value = serde_json::json!({
        "schema_version": 1,
        "host": "postgres.database.svc.cluster.local",
        "postgres_port": 5432,
        "redis_port": 6379,
        "neo4j_http_port": 7474,
        "postgres_user": "nexora",
        "postgres_database": "nexora",
        "secrets_directory": "/run/secrets/nexora"
    });
    let config: DatabaseConnections = serde_json::from_value(value.clone()).unwrap();
    assert!(config.validate().is_err());

    value["allow_remote_host"] = serde_json::json!(true);
    let config: DatabaseConnections = serde_json::from_value(value.clone()).unwrap();
    config.validate().unwrap();

    value["host"] = serde_json::json!("postgres..database");
    let config: DatabaseConnections = serde_json::from_value(value).unwrap();
    assert!(config.validate().is_err());
}
