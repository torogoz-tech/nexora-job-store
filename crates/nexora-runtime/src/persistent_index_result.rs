use serde::Serialize;
#[derive(Serialize)]
pub struct PersistentIndexResult {
    pub snapshot: nexora_storage::Snapshot,
    pub reuse: nexora_indexer::ReuseStatistics,
    pub operation_ms: f64,
    pub storage_ms: f64,
    pub timings: nexora_indexer::StageTimings,
}
