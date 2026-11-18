use crate::SecretGenerator;
use nexora_platform::{PlatformError, ProjectDocumentVersion};
use serde_json::{Value, json};

pub(crate) struct DocumentVersionChecksum;
impl DocumentVersionChecksum {
    pub fn calculate(version: &ProjectDocumentVersion) -> Result<String, PlatformError> {
        // State and publication evolve separately; they never alter the content identity.
        let mut value = json!({"id":version.id,"project_id":version.project_id,"document_id":version.document_id,"number":version.number,"previous_version_id":version.previous_version_id,"title":version.title,"content":version.content,"origin":version.origin,"workflow_version_id":version.workflow_version_id,"created_by":version.created_by,"reason":version.reason});
        if let Some(id) = version.context_manifest_id.as_deref() {
            value["context_manifest_id"] = json!(id);
            value["evidence"] =
                serde_json::to_value(&version.evidence).map_err(|_| PlatformError::Storage)?;
        }
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
