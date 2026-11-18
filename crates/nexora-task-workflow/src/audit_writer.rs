use crate::{SecretGenerator, TaskWorkflowStore};
use nexora_platform::PlatformError;
use serde_json::Value;
pub(crate) struct AuditWriter;
impl AuditWriter {
    pub fn append(
        client: &mut impl postgres::GenericClient,
        actor: Option<&str>,
        credential: Option<&str>,
        operation: &str,
        resource: Option<&str>,
        metadata: Value,
    ) -> Result<(), PlatformError> {
        let id = SecretGenerator::id("evt")?;
        client.execute("INSERT INTO audit_events(id,actor_id,credential_id,operation,resource_id,metadata) VALUES($1,$2,$3,$4,$5,$6)",&[&id,&actor,&credential,&operation,&resource,&metadata]).map_err(TaskWorkflowStore::database_error)?;
        Ok(())
    }
}
