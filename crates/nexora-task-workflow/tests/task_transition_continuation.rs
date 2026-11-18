use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use std::path::Path;

/// Uses a separately provisioned database with the API-owned migration set applied.
#[test]
#[ignore = "requires isolated PostgreSQL with API migration 0088 applied"]
fn task_transition_session_lock_serializes_worker_pods() {
    let settings_path = std::env::var("NEXORA_TASK_WORKFLOW_TEST_DATABASE")
        .expect("set NEXORA_TASK_WORKFLOW_TEST_DATABASE to a private connection-settings file");
    let settings = DatabaseConnections::load(Path::new(&settings_path)).unwrap();
    let mut first = PostgresConnection::connect(&settings).unwrap();
    let mut second = PostgresConnection::connect(&settings).unwrap();
    let namespace = std::env::var("NEXORA_TASK_WORKFLOW_TEST_NAMESPACE")
        .unwrap_or_else(|_| "nexora_platform_test".into());
    assert!(namespace.starts_with("nexora_platform"));
    first
        .client()
        .batch_execute(&format!(
            "SET search_path TO {namespace},nexora_platform,pg_catalog"
        ))
        .unwrap();
    second
        .client()
        .batch_execute(&format!(
            "SET search_path TO {namespace},nexora_platform,pg_catalog"
        ))
        .unwrap();
    let schema_ready: bool = first
        .client()
        .query_one(
            "SELECT to_regclass('task_transition_runs') IS NOT NULL",
            &[],
        )
        .unwrap()
        .get(0);
    assert!(schema_ready, "API-owned migration 0088 must be applied");

    let lock_name = "task-transition:task-workflow-integration-lock";
    let acquired: bool = first
        .client()
        .query_one(
            "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
            &[&lock_name],
        )
        .unwrap()
        .get(0);
    assert!(acquired);
    let competing_acquired: bool = second
        .client()
        .query_one(
            "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
            &[&lock_name],
        )
        .unwrap()
        .get(0);
    assert!(!competing_acquired);

    let released: bool = first
        .client()
        .query_one(
            "SELECT pg_advisory_unlock(hashtextextended($1,0))",
            &[&lock_name],
        )
        .unwrap()
        .get(0);
    assert!(released);
    let retry_acquired: bool = second
        .client()
        .query_one(
            "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
            &[&lock_name],
        )
        .unwrap()
        .get(0);
    assert!(retry_acquired);
    second
        .client()
        .query_one(
            "SELECT pg_advisory_unlock(hashtextextended($1,0))",
            &[&lock_name],
        )
        .unwrap();
}
