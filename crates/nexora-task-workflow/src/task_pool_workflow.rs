use crate::{SecretGenerator, TaskWorkflowStore};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::json;
pub(crate) struct TaskPoolWorkflow;
impl TaskPoolWorkflow {
    pub fn flow(
        tx: &mut Transaction<'_>,
        project: Option<&str>,
        id: &str,
        fresh: bool,
    ) -> Result<WorkflowVersion, PlatformError> {
        let value:serde_json::Value=tx.query_opt("SELECT to_jsonb(v) FROM workflow_versions v JOIN workflows w ON w.id=v.workflow_id WHERE v.id=$1 AND (v.project_id IS NULL OR v.project_id=$2) AND (NOT $3 OR NOT w.archived)",&[&id,&project,&fresh]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?.get(0);
        let version: WorkflowVersion = TaskWorkflowStore::decode(value)?;
        if !version.resource_kinds.contains(&WorkflowResourceKind::Task) {
            return Err(PlatformError::Validation("workflow does not govern tasks"));
        }
        version.graph.validate()?;
        let content = json!({"name":version.name,"purpose":version.purpose,"resource_kinds":version.resource_kinds,"graph":version.graph});
        if SecretGenerator::checksum(
            &serde_json::to_vec(&content).map_err(|_| PlatformError::Storage)?,
        ) != version.checksum
        {
            return Err(PlatformError::Conflict("workflow checksum changed"));
        }
        Ok(version)
    }
}
