use nexora_core::{RepositoryId, SnapshotId, TenantId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuerySnapshot {
    pub tenant_id: TenantId,
    pub repository_id: RepositoryId,
    pub id: SnapshotId,
    pub payload_hash: String,
    pub summary: Value,
}
impl QuerySnapshot {
    pub const VERSION: i32 = 3;
    pub fn identity(&self) -> Value {
        json!({"tenant":self.tenant_id.0,"repository":self.repository_id.0,"snapshot":self.id.0})
    }
    pub(crate) fn page(&self, items: Vec<Value>, total: usize, offset: usize) -> Value {
        let next = offset.saturating_add(items.len());
        let mut result = self.identity();
        result["total"] = json!(total);
        result["next_offset"] = if next < total {
            json!(next)
        } else {
            Value::Null
        };
        result["truncated"] = json!(next < total);
        result["items"] = json!(items);
        result
    }
}
