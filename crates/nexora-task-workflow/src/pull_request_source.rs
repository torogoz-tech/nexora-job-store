use crate::{
    GitNetworkPolicy, TaskWorkflowStore, platform_authority::PlatformAuthority,
    repository_job_source::RepositoryJobSource,
};
use nexora_platform::*;
pub(crate) struct PullRequestSource {
    pub repository: ProjectRepository,
    pub connection: GitConnection,
    pub credential_revision: i64,
}
impl PullRequestSource {
    pub fn load(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
        policy: &GitNetworkPolicy,
    ) -> Result<Self, PlatformError> {
        policy.validate()?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        PlatformAuthority::require(&mut tx, ctx, Capability::RepositoriesRead, Some(project))?;
        PlatformAuthority::project(&mut tx, project)?;
        let row=tx.query_opt("SELECT repository_metadata(r) FROM project_repositories r WHERE id=$1 AND project_id=$2 AND NOT archived",&[&repository,&project]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
        let metadata: ProjectRepository = TaskWorkflowStore::decode(row.get(0))?;
        if metadata.source_kind != RepositorySourceKind::Git {
            return Err(PlatformError::Validation(
                "pull requests require an authorized Git repository",
            ));
        }
        let source =
            RepositoryJobSource::lock(&mut tx, ctx, project, repository, metadata.revision)?;
        let row=tx.query_one("SELECT to_jsonb(c)-'tokens',credential_revision FROM git_connections c WHERE id=$1",&[&source.connection]).map_err(TaskWorkflowStore::database_error)?;
        let connection: GitConnection = TaskWorkflowStore::decode(row.get(0))?;
        let host = match connection.provider {
            GitProvider::Github => policy.github_host()?,
            GitProvider::Gitlab => policy.host()?,
        };
        if metadata.provider != Some(connection.provider)
            || metadata.host.as_deref() != Some(host.as_str())
            || connection.host != host
            || metadata.remote_id.is_none()
            || metadata.remote_full_name.is_none()
        {
            return Err(PlatformError::Conflict(
                "repository provider identity changed",
            ));
        }
        let result = Self {
            repository: metadata,
            connection,
            credential_revision: row.get(1),
        };
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(result)
    }
    pub fn matches(&self, other: &Self) -> bool {
        self.repository.revision == other.repository.revision
            && self.repository.remote_id == other.repository.remote_id
            && self.repository.source_generation == other.repository.source_generation
            && self.connection.id == other.connection.id
            && self.connection.revision == other.connection.revision
            && self.credential_revision == other.credential_revision
    }
}
