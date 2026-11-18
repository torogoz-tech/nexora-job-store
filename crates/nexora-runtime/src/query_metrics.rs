use serde::Serialize;
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueryMetrics {
    pub cache_hit: bool,
    pub cache_available: bool,
    pub backend: String,
    pub metadata_us: u64,
    pub cache_us: u64,
    pub backend_us: u64,
}
