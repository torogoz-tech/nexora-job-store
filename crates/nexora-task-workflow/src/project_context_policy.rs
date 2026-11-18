use crate::{TaskWorkflowStore, project_context_checksum::ProjectContextChecksum};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::Value;
pub(crate) struct ProjectContextPolicy;
impl ProjectContextPolicy {
    pub fn get(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
    ) -> Result<ProjectContextManifest, PlatformError> {
        let value: Value = tx
            .query_opt(
                "SELECT to_jsonb(m) FROM project_context_manifests m WHERE id=$1 AND project_id=$2",
                &[&id, &project],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let result: ProjectContextManifest = TaskWorkflowStore::decode(value)?;
        if result.members.is_empty()
            || result.members.len() > 64
            || result
                .members
                .windows(2)
                .any(|pair| pair[0].repository_id >= pair[1].repository_id)
            || ProjectContextChecksum::manifest(&result)? != result.checksum
        {
            return Err(PlatformError::Conflict("context manifest checksum changed"));
        }
        Ok(result)
    }
    pub fn status(
        tx: &mut Transaction<'_>,
        manifest: &ProjectContextManifest,
    ) -> Result<ProjectContextStatus, PlatformError> {
        let value: serde_json::Value = tx
            .query_one(
                "SELECT project_context_status($1,$2)",
                &[&manifest.project_id, &manifest.id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        TaskWorkflowStore::decode(value)
    }
    pub fn lock_sources(
        tx: &mut Transaction<'_>,
        manifest: &ProjectContextManifest,
    ) -> Result<(), PlatformError> {
        // Source locks precede document row locks, matching repository publication order.
        for member in &manifest.members {
            if let Some(origin) = &member.local_origin {
                let row = tx.query_opt("SELECT owner_actor_id FROM local_repository_sources WHERE id=$1 AND repository_id=$2 AND project_id=$3 FOR SHARE", &[&origin.source_id,&member.repository_id,&manifest.project_id]).map_err(TaskWorkflowStore::database_error)?;
                if let Some(row) = row {
                    let owner: String = row.get(0);
                    tx.query_opt("SELECT id FROM actors WHERE id=$1 FOR SHARE", &[&owner])
                        .map_err(TaskWorkflowStore::database_error)?;
                }
                tx.query_opt(
                    "SELECT id FROM project_repositories WHERE id=$1 AND project_id=$2 FOR SHARE",
                    &[&member.repository_id, &manifest.project_id],
                )
                .map_err(TaskWorkflowStore::database_error)?;
                tx.query_opt("SELECT id FROM local_snapshot_publications WHERE id=$1 AND source_id=$2 FOR SHARE", &[&origin.publication_id,&origin.source_id]).map_err(TaskWorkflowStore::database_error)?;
                continue;
            }
            let row = tx
                .query_opt(
                    "SELECT owner_actor_id FROM git_connections WHERE id=$1 FOR SHARE",
                    &[&member.connection_id],
                )
                .map_err(TaskWorkflowStore::database_error)?;
            if let Some(row) = row {
                let owner: String = row.get(0);
                tx.query_opt("SELECT id FROM actors WHERE id=$1 FOR SHARE", &[&owner])
                    .map_err(TaskWorkflowStore::database_error)?;
            }
            tx.query_opt(
                "SELECT id FROM project_repositories WHERE id=$1 AND project_id=$2 FOR SHARE",
                &[&member.repository_id, &manifest.project_id],
            )
            .map_err(TaskWorkflowStore::database_error)?;
            tx.query_opt("SELECT repository_id FROM repository_bindings WHERE repository_id=$1 AND project_id=$2 FOR SHARE",&[&member.repository_id,&manifest.project_id]).map_err(TaskWorkflowStore::database_error)?;
        }
        Ok(())
    }
}
