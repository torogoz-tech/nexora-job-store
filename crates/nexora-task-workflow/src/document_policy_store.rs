use crate::{
    TaskWorkflowStore, document_version_checksum::DocumentVersionChecksum,
    platform_authority::PlatformAuthority,
};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::Value;

pub(crate) struct DocumentPolicyStore;
impl DocumentPolicyStore {
    pub fn authorize(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        cap: Capability,
    ) -> Result<(), PlatformError> {
        PlatformAuthority::require(tx, ctx, cap, Some(project)).map_err(|error| {
            if cap == Capability::DocumentsRead && matches!(error, PlatformError::Forbidden) {
                PlatformError::NotFound
            } else {
                error
            }
        })?;
        if cap == Capability::DocumentsWrite {
            tx.query_opt("SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','pmo','manager','super_admin') FOR SHARE",&[&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        }
        PlatformAuthority::project(tx, project)
    }
    pub fn definition(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
        lock: bool,
    ) -> Result<ProjectDocument, PlatformError> {
        let sql = if lock {
            "SELECT to_jsonb(d) FROM project_documents d WHERE id=$1 AND project_id=$2 FOR UPDATE"
        } else {
            "SELECT to_jsonb(d) FROM project_documents d WHERE id=$1 AND project_id=$2"
        };
        let value: Value = tx
            .query_opt(sql, &[&id, &project])
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        TaskWorkflowStore::decode(value)
    }
    pub fn version(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
        version: &str,
    ) -> Result<ProjectDocumentVersion, PlatformError> {
        let value:Value=tx.query_opt("SELECT to_jsonb(v)||jsonb_build_object('state_id',s.state_id,'state_revision',s.state_revision) FROM project_document_versions v JOIN document_version_states s ON s.version_id=v.id WHERE v.id=$1 AND v.project_id=$2 AND v.document_id=$3",&[&version,&project,&id]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?.get(0);
        let mut result: ProjectDocumentVersion = TaskWorkflowStore::decode(value)?;
        if DocumentVersionChecksum::calculate(&result)? != result.checksum {
            return Err(PlatformError::Conflict("document checksum changed"));
        }
        if let Some(id) = result.context_manifest_id.as_deref() {
            let manifest =
                crate::project_context_policy::ProjectContextPolicy::get(tx, project, id)?;
            result.needs_review =
                crate::project_context_policy::ProjectContextPolicy::status(tx, &manifest)?
                    .needs_review;
        }
        Ok(result)
    }
}
