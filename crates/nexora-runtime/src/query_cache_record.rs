use crate::bounded_json::BoundedJson;
use nexora_core::NexoraError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryCacheRecord {
    pub key: String,
    pub data: Value,
    pub data_hash: String,
    pub computation_us: u64,
}
impl QueryCacheRecord {
    pub fn new(key: String, data: Value, computation_us: u64) -> Result<Self, NexoraError> {
        let bytes = BoundedJson::encode(&data, 1_048_576)?;
        Ok(Self {
            key,
            data,
            data_hash: format!("{:x}", Sha256::digest(bytes)),
            computation_us,
        })
    }
    pub fn valid(&self, key: &str) -> bool {
        self.key == key
            && BoundedJson::encode(&self.data, 1_048_576)
                .is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == self.data_hash)
    }
}
