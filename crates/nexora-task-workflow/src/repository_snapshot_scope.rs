use crate::{
    TaskWorkflowStore, repository_access::RepositoryAccess,
    repository_job_source::RepositoryJobSource,
};
use nexora_platform::{AuthContext, PlatformError, RepositorySourceKind};
use postgres::Transaction;
use serde_json::{Value, json};

/// Reads and locks the source facts needed to validate an existing task work session.
pub(crate) struct RepositorySnapshotScope {
    authority: Value,
}

impl RepositorySnapshotScope {
    pub(crate) fn lock(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
        snapshot: Option<&str>,
    ) -> Result<Self, PlatformError> {
        if snapshot.is_some_and(|id| id.is_empty() || id.len() > 4096) {
            return Err(PlatformError::Validation("invalid snapshot identifier"));
        }
        let access = RepositoryAccess::lock(tx, ctx, project, repository)?;
        let authority = match access.source_kind {
            RepositorySourceKind::Git => {
                let metadata: Value = tx
                    .query_opt(
                        "SELECT repository_metadata(r) FROM project_repositories r WHERE id=$1 AND project_id=$2 AND NOT archived FOR SHARE",
                        &[&repository, &project],
                    )
                    .map_err(TaskWorkflowStore::database_error)?
                    .ok_or(PlatformError::NotFound)?
                    .get(0);
                let binding = tx
                    .query_opt(
                        "SELECT runtime_tenant_id,runtime_repository_id,snapshot_id,pinned_commit,payload_hash FROM repository_bindings WHERE repository_id=$1 AND project_id=$2 FOR SHARE",
                        &[&repository, &project],
                    )
                    .map_err(TaskWorkflowStore::database_error)?
                    .ok_or(PlatformError::NotFound)?;
                let source =
                    RepositoryJobSource::lock(tx, ctx, project, repository, access.revision)?;
                let selected_snapshot = snapshot
                    .map(str::to_owned)
                    .unwrap_or_else(|| binding.get(2));
                if selected_snapshot != binding.get::<_, String>(2) {
                    tx.query_opt("SELECT j.id FROM repository_jobs j JOIN repository_bindings b ON b.repository_id=j.repository_id AND b.project_id=j.project_id AND b.source_generation=j.source_generation AND b.connection_id=j.connection_id AND b.connection_revision=j.connection_revision WHERE j.project_id=$1 AND j.repository_id=$2 AND j.snapshot_id=$3 AND j.state='succeeded' ORDER BY j.finished_at,j.id LIMIT 1",&[&project,&repository,&selected_snapshot]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
                }
                json!({
                    "tenant": binding.get::<_, String>(0),
                    "repository": binding.get::<_, String>(1),
                    "source_generation": source.generation,
                    "connection_id": source.connection,
                    "connection_revision": source.connection_revision,
                    "host": metadata["host"],
                    "remote_full_name": metadata["remote_full_name"],
                })
            }
            RepositorySourceKind::LocalSnapshot => {
                let source=tx.query_opt("SELECT id,owner_actor_id,runtime_tenant_id,runtime_repository_id,authorization_revision,source_generation,binding_revision,current_publication_id FROM local_repository_sources WHERE repository_id=$1 AND project_id=$2 AND state='active' FOR SHARE",&[&repository,&project]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
                let source_id: String = source.get(0);
                let owner: String = source.get(1);
                let tenant: String = source.get(2);
                let runtime_repository: String = source.get(3);
                let epoch: i64 = source.get(4);
                let generation: i64 = source.get(5);
                let publication_id: String = source.get(7);
                tx.query_opt(
                    "SELECT id FROM actors WHERE id=$1 AND active FOR SHARE",
                    &[&owner],
                )
                .map_err(TaskWorkflowStore::database_error)?
                .ok_or(PlatformError::NotFound)?;
                tx.query_opt("SELECT id FROM project_repositories WHERE id=$1 AND project_id=$2 AND revision=$3 AND source_kind='local_snapshot' AND source_generation=$4 AND NOT archived AND authorization_status='authorized' FOR SHARE",&[&repository,&project,&access.revision,&generation]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
                let _publication=tx.query_opt("SELECT id FROM local_snapshot_publications WHERE source_id=$1 AND authorization_revision=$2 AND (($3::text IS NULL AND id=$4) OR ($3::text IS NOT NULL AND snapshot_id=$3)) ORDER BY binding_revision DESC,id LIMIT 1 FOR SHARE",&[&source_id,&epoch,&snapshot,&publication_id]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
                json!({"tenant":tenant,"repository":runtime_repository,"source_kind":"local_snapshot","source_id":source_id,"source_generation":generation,"authorization_revision":epoch,"owner_actor_id":owner})
            }
        };
        Ok(Self { authority })
    }

    pub(crate) fn authority(&self, _tx: &mut Transaction<'_>) -> Result<Value, PlatformError> {
        Ok(self.authority.clone())
    }
}
