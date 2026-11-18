use nexora_database_connections::DatabaseConnections;
use nexora_platform::PlatformError;
use nexora_task_workflow::TaskWorkflowStore;
use std::path::PathBuf;

fn invalid_settings() -> DatabaseConnections {
    DatabaseConnections {
        schema_version: 1,
        host: "127.0.0.1".into(),
        postgres_port: 0,
        redis_port: 6379,
        neo4j_http_port: 7474,
        postgres_database: "nexora".into(),
        postgres_user: "nexora".into(),
        secrets_directory: PathBuf::from("unused"),
        allow_remote_host: false,
    }
}

#[test]
fn rejects_invalid_namespaces_before_opening_a_connection() {
    let oversized = format!("nexora_platform_{}", "a".repeat(48));
    let invalid = [
        "",
        "public",
        "nexora_platform;DROP SCHEMA public",
        "nexora_platform,public",
        "nexora_platform_MixedCase",
        "nexora_platform-á",
        &oversized,
    ];

    for namespace in invalid {
        assert!(
            matches!(
                TaskWorkflowStore::connect(&invalid_settings(), namespace),
                Err(PlatformError::Validation("invalid platform namespace"))
            ),
            "{namespace:?} must be rejected before database access"
        );
    }
}

#[test]
fn accepts_platform_namespaces_up_to_the_postgres_identifier_limit() {
    let longest = format!("nexora_platform_{}", "a".repeat(47));

    for namespace in ["nexora_platform", "nexora_platform_test_01", &longest] {
        assert!(
            matches!(
                TaskWorkflowStore::connect(&invalid_settings(), namespace),
                Err(PlatformError::Validation("invalid database configuration"))
            ),
            "{namespace:?} must reach database configuration validation"
        );
    }
}
