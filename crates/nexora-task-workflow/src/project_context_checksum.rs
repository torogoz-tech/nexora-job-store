use crate::SecretGenerator;
use nexora_platform::{PlatformError, ProjectContextManifest};
use serde_json::{Value, json};
pub(crate) struct ProjectContextChecksum;
impl ProjectContextChecksum {
    pub fn manifest(value: &ProjectContextManifest) -> Result<String, PlatformError> {
        Self::value(
            json!({"id":value.id,"project_id":value.project_id,"name":value.name,"members":value.members,"reason":value.reason,"created_by":value.created_by}),
        )
    }
    pub fn value(value: Value) -> Result<String, PlatformError> {
        Ok(SecretGenerator::checksum(
            &serde_json::to_vec(&Self::canonical(value)).map_err(|_| PlatformError::Storage)?,
        ))
    }
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut entries = fields.into_iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, Self::canonical(value)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.into_iter().map(Self::canonical).collect()),
            other => other,
        }
    }
}
