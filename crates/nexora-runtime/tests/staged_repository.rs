mod common;

use nexora_core::{RepositoryId, TenantId};
use nexora_indexer::Indexer;
use nexora_language::LanguageDefinition;
use nexora_runtime::{DatabaseConnections, DatabaseRuntime};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
#[ignore = "requires PostgreSQL, Dragonfly and Neo4j; prepared snapshots must not activate repository heads"]
fn staged_repository_is_queryable_by_id_without_replacing_the_active_head() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-staged-runtime-{nonce}"));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("source.js");
    std::fs::write(&source, "function previous() {}\n").unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("platform-staged-{nonce}"));
    let repository = RepositoryId(format!("repo_staged_{nonce}"));
    let unbound = RepositoryId(format!("repo_unbound_{nonce}"));
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let active = runtime
        .index_repository(&indexer, &root, tenant.clone(), repository.clone())
        .unwrap();
    let active_header = runtime.query_header(&tenant, &repository, None).unwrap();
    std::fs::write(&source, "function prepared() {}\n").unwrap();
    let staged = runtime
        .stage_repository(&indexer, &root, tenant.clone(), repository.clone())
        .unwrap();
    assert_ne!(staged.snapshot.id, active.snapshot.id);
    let staged_id = staged.snapshot.id.clone();
    let staged_header = runtime
        .query_header(&tenant, &repository, Some(&staged_id))
        .unwrap();
    assert_eq!(
        runtime.query_header(&tenant, &repository, None).unwrap().id,
        active.snapshot.id
    );
    assert_eq!(
        runtime
            .query_header(&tenant, &repository, None)
            .unwrap()
            .payload_hash,
        active_header.payload_hash
    );
    let first = runtime
        .stage_repository(&indexer, &root, tenant.clone(), unbound.clone())
        .unwrap();
    let first_id = first.snapshot.id.clone();
    assert!(runtime.snapshot(&tenant, &unbound, None).unwrap().is_none());
    assert!(runtime.query_header(&tenant, &unbound, None).is_err());
    drop(runtime);
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    assert_eq!(
        runtime
            .snapshot(&tenant, &repository, None)
            .unwrap()
            .unwrap()
            .id,
        active.snapshot.id
    );
    assert_eq!(
        runtime
            .query_header(&tenant, &repository, Some(&staged_id))
            .unwrap()
            .payload_hash,
        staged_header.payload_hash
    );
    assert_eq!(
        serde_json::to_value(
            runtime
                .snapshot(&tenant, &repository, Some(&staged_id))
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&staged.snapshot).unwrap()
    );
    assert!(runtime.snapshot(&tenant, &unbound, None).unwrap().is_none());
    assert_eq!(
        runtime
            .snapshot(&tenant, &unbound, Some(&first_id))
            .unwrap()
            .unwrap()
            .id,
        first_id
    );
    assert!(
        runtime
            .snapshot(
                &TenantId(format!("other-{nonce}")),
                &repository,
                Some(&staged_id)
            )
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(root).unwrap();
}
