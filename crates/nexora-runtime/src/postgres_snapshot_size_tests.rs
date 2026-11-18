use crate::{DatabaseConnections, PostgresSnapshots, connection_secrets::ConnectionSecrets};
use nexora_core::{RepositoryId, SnapshotId, TenantId};
use nexora_storage::Snapshot;
use std::{collections::BTreeMap, time::SystemTime};

#[test]
#[ignore = "requires local PostgreSQL and memory for a snapshot larger than 256 MiB"]
fn postgres_round_trips_snapshots_larger_than_the_old_limit() {
    let config =
        DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
            .unwrap();
    let secrets = ConnectionSecrets::load(&config).unwrap();
    let mut store = PostgresSnapshots::connect(&config, &secrets).unwrap();
    store.initialize().unwrap();
    let namespace = format!(
        "snapshot-size-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let snapshot = Snapshot {
        tenant_id: TenantId(namespace),
        repository_id: RepositoryId("large-repository".into()),
        id: SnapshotId("large-snapshot".into()),
        graph: Default::default(),
        sources: BTreeMap::from([("large.txt".into(), "x".repeat(256 * 1024 * 1024 + 1))]),
        diagnostics: vec![],
        rule_version: "size-regression".into(),
        analysis_engine: None,
        analysis_configuration: None,
        source_coverage: vec![],
    };
    let hash = store
        .prepare(&snapshot)
        .expect("valid large snapshots must be persisted");
    store.ready(&snapshot, &hash, true).unwrap();
    let restored = store
        .get(&snapshot.tenant_id, &snapshot.repository_id, None)
        .unwrap()
        .unwrap();
    assert_eq!(restored.sources, snapshot.sources);
    assert_eq!(restored.id, snapshot.id);
    store
        .client()
        .execute(
            "UPDATE nexora_runtime.snapshots SET payload_hash=$2 WHERE tenant_id=$1",
            &[&snapshot.tenant_id.0, &"invalid"],
        )
        .unwrap();
    assert!(
        matches!(
            store.get(&snapshot.tenant_id, &snapshot.repository_id, None),
            Err(nexora_core::NexoraError::Storage(_))
        ),
        "large snapshots must still be checked for corruption"
    );
    store
        .client()
        .execute(
            "UPDATE nexora_runtime.repositories SET current_snapshot_id=NULL WHERE tenant_id=$1",
            &[&snapshot.tenant_id.0],
        )
        .unwrap();
    store
        .client()
        .execute(
            "DELETE FROM nexora_runtime.snapshots WHERE tenant_id=$1",
            &[&snapshot.tenant_id.0],
        )
        .unwrap();
    store
        .client()
        .execute(
            "DELETE FROM nexora_runtime.repositories WHERE tenant_id=$1",
            &[&snapshot.tenant_id.0],
        )
        .unwrap();
}
