mod common;

use nexora_core::{RepositoryId, TenantId};
use nexora_indexer::Indexer;
use nexora_language::LanguageDefinition;
use nexora_runtime::{DatabaseConnections, DatabaseRuntime};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
#[ignore = "requires PostgreSQL, Dragonfly and Neo4j; stable identity through persisted reindex and restart"]
fn registered_repository_survives_checkout_relocation_and_runtime_restart() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let parent = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-registered-runtime-{nonce}"));
    let original = parent.join("original");
    let relocated = parent.join("relocated");
    std::fs::create_dir_all(&original).unwrap();
    std::fs::write(original.join("source.js"), "function greet() {}\n").unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("platform-bridge-{nonce}"));
    let repository = RepositoryId(format!("repo_bridge_{nonce}"));
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let cold = runtime
        .index_repository(&indexer, &original, tenant.clone(), repository.clone())
        .unwrap();
    assert_eq!(cold.reuse.analyzed_files, 1);
    let header = runtime.query_header(&tenant, &repository, None).unwrap();
    std::fs::rename(&original, &relocated).unwrap();
    drop(runtime);
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let warm = runtime
        .index_repository(&indexer, &relocated, tenant.clone(), repository.clone())
        .unwrap();
    assert_eq!(warm.reuse.reused_files, 1);
    assert_eq!(warm.snapshot.id, cold.snapshot.id);
    assert_eq!(
        serde_json::to_value(warm.snapshot).unwrap(),
        serde_json::to_value(cold.snapshot).unwrap()
    );
    assert_eq!(
        runtime
            .query_header(&tenant, &repository, None)
            .unwrap()
            .payload_hash,
        header.payload_hash
    );
    assert!(
        runtime
            .snapshot(&tenant, &repository, None)
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(parent).unwrap();
}
