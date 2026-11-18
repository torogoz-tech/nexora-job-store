use crate::{snapshot_checksum::SnapshotChecksum, snapshot_payload::SnapshotPayload};
use nexora_core::{RepositoryId, SnapshotId, TenantId};
use nexora_storage::Snapshot;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[test]
fn compressed_and_historical_payloads_keep_the_same_content_identity() {
    let snapshot = Snapshot {
        tenant_id: TenantId("tenant".into()),
        repository_id: RepositoryId("repo".into()),
        id: SnapshotId("snapshot".into()),
        graph: Default::default(),
        sources: BTreeMap::from([("source.txt".into(), "Unicode á\0\n".repeat(1000))]),
        diagnostics: vec![],
        rule_version: "v1".into(),
        analysis_engine: None,
        analysis_configuration: None,
        source_coverage: vec![],
    };
    let historical = serde_json::to_vec(&snapshot).unwrap();
    let compressed = SnapshotPayload::encode(&snapshot).unwrap();
    assert!(compressed.len() < historical.len());
    let expected = format!("{:x}", Sha256::digest(&historical));
    for payload in [&historical, &compressed] {
        let restored = SnapshotPayload::decode(payload).unwrap();
        assert_eq!(restored.sources, snapshot.sources);
        assert_eq!(SnapshotChecksum::calculate(&restored).unwrap(), expected);
    }
    assert!(SnapshotPayload::decode(&compressed[..compressed.len() / 2]).is_err());
}
