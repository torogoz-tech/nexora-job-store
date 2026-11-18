use crate::{
    AnalysisStore, GitNetworkPolicy, RepositoryJobLease,
    repository_job_execution_authority::RepositoryJobExecutionAuthority,
};
use nexora_platform::{GitConnection, GitProvider, PlatformError, RepositoryJob};

pub(crate) struct RepositoryJobSourceRequest {
    pub job: RepositoryJob,
    pub connection: GitConnection,
    pub remote_id: String,
    pub remote_name: String,
    pub credential_revision: i64,
}
impl RepositoryJobSourceRequest {
    pub fn load(
        store: &mut AnalysisStore,
        lease: &RepositoryJobLease,
        policy: &GitNetworkPolicy,
    ) -> Result<Self, PlatformError> {
        policy.validate()?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row = RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
        let value: serde_json::Value = row.get(0);
        let job: RepositoryJob = AnalysisStore::decode(value.clone())?;
        let connection_id = value["connection_id"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let row = tx.query_one("SELECT to_jsonb(c)-'tokens',credential_revision FROM git_connections c WHERE id=$1", &[&connection_id]).map_err(AnalysisStore::database_error)?;
        let connection: GitConnection = AnalysisStore::decode(row.get(0))?;
        let credential_revision = row.get(1);
        let row = tx.query_one("SELECT remote_id,remote_full_name,provider,host FROM project_repositories WHERE id=$1 AND project_id=$2", &[&job.repository_id,&job.project_id]).map_err(AnalysisStore::database_error)?;
        let provider: String = row.get(2);
        let host: String = row.get(3);
        let policy_host = match connection.provider {
            GitProvider::Github => policy.github_host()?,
            GitProvider::Gitlab => policy.host()?,
        };
        if provider != connection.provider.as_str()
            || host != connection.host
            || host != policy_host
        {
            return Err(PlatformError::Conflict(
                "repository provider policy changed",
            ));
        }
        let request = Self {
            job,
            connection,
            remote_id: row
                .get::<_, Option<String>>(0)
                .ok_or(PlatformError::Storage)?,
            remote_name: row
                .get::<_, Option<String>>(1)
                .ok_or(PlatformError::Storage)?,
            credential_revision,
        };
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(request)
    }
}
