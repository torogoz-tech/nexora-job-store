use crate::{
    TaskWorkflowStore, development_policy_store::DevelopmentPolicyStore,
    platform_authority::PlatformAuthority, repository_job_source::RepositoryJobSource,
};
use nexora_platform::{AuthContext, Capability, PlatformError};
use serde_json::{Value, json};

/// Reads live repository/provider authority facts using the existing API-owned schema.
pub(crate) struct TaskTransitionProviderAuthority;

impl TaskTransitionProviderAuthority {
    pub(crate) fn lock(
        transaction: &mut postgres::Transaction<'_>,
        context: &AuthContext,
        project_id: &str,
        repository_id: &str,
    ) -> Result<Value, PlatformError> {
        PlatformAuthority::require(
            transaction,
            context,
            Capability::RepositoriesRead,
            Some(project_id),
        )?;
        PlatformAuthority::project(transaction, project_id)?;
        let metadata: Value = transaction
            .query_opt(
                "SELECT repository_metadata(r) FROM project_repositories r WHERE id=$1 AND project_id=$2 AND NOT archived",
                &[&repository_id, &project_id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let repository: nexora_platform::ProjectRepository =
            TaskWorkflowStore::decode(metadata.clone())?;
        let source = RepositoryJobSource::lock(
            transaction,
            context,
            project_id,
            repository_id,
            repository.revision,
        )?;
        let credential_revision: i64 = transaction
            .query_opt(
                "SELECT credential_revision FROM git_connections WHERE id=$1 AND status='active' FOR SHARE",
                &[&source.connection],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict("repository source is not authorized"))?
            .get(0);
        let policy = DevelopmentPolicyStore::lock(transaction, project_id, repository_id)?;
        Ok(json!({
            "repository_id": repository_id,
            "repository_revision": repository.revision,
            "remote_id": repository.remote_id,
            "remote_full_name": repository.remote_full_name,
            "provider": repository.provider,
            "host": repository.host,
            "source_generation": source.generation,
            "connection_id": source.connection,
            "connection_revision": source.connection_revision,
            "credential_revision": credential_revision,
            "policy_revision": policy.revision,
        }))
    }
}
