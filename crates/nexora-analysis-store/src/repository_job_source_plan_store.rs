use crate::{
    AnalysisStore, GitNetworkPolicy, RepositoryJobLease, RepositoryJobSourcePlan,
    audit_writer::AuditWriter, github_client::GithubClient,
    github_repository_proof::GithubRepositoryProof, gitlab_client::GitlabClient,
    gitlab_token_store::GitlabTokenStore,
    repository_job_execution_authority::RepositoryJobExecutionAuthority,
    repository_job_source_request::RepositoryJobSourceRequest,
};
use nexora_platform::{GitProvider, PlatformError};
use nexora_source::GitFetchCredentials;
use serde_json::json;

pub(crate) struct RepositoryJobSourcePlanStore<'a> {
    store: &'a mut AnalysisStore,
}
impl<'a> RepositoryJobSourcePlanStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }
    fn prove(
        &mut self,
        request: &RepositoryJobSourceRequest,
        policy: &GitNetworkPolicy,
    ) -> Result<(String, String, GitFetchCredentials), PlatformError> {
        match request.connection.provider {
            GitProvider::Gitlab => {
                let mut tokens =
                    GitlabTokenStore::new(self.store).tokens(&request.connection, policy)?;
                let client = GitlabClient::new(policy);
                if client.user(&tokens.access_token)?.0 != request.connection.remote_account_id {
                    return Err(PlatformError::Unauthorized);
                }
                let (id, name) = client.repository(&tokens.access_token, &request.remote_id)?;
                if id != request.remote_id {
                    return Err(PlatformError::Conflict(
                        "registered repository identity changed",
                    ));
                }
                let commit = client.commit(
                    &tokens.access_token,
                    &id,
                    request.job.pinned_commit.as_deref(),
                )?;
                let credentials =
                    GitFetchCredentials::new("oauth2", std::mem::take(&mut tokens.access_token))
                        .map_err(|_| PlatformError::Unavailable)?;
                Ok((name, commit, credentials))
            }
            GitProvider::Github => {
                let (_, token) = GithubRepositoryProof::check(
                    self.store,
                    &request.connection,
                    &request.remote_name,
                    Some(&request.remote_id),
                    policy,
                )?;
                let commit = GithubClient::new(policy).commit(
                    &token.access_token,
                    &token.full_name,
                    request.job.pinned_commit.as_deref(),
                )?;
                let credentials =
                    GitFetchCredentials::new("x-access-token", token.access_token.to_string())
                        .map_err(|_| PlatformError::Unavailable)?;
                Ok((token.full_name, commit, credentials))
            }
        }
    }
    fn reject(
        &mut self,
        lease: &RepositoryJobLease,
        request: &RepositoryJobSourceRequest,
        error: &PlatformError,
    ) -> Result<(), PlatformError> {
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
        // Provider HTTP failures are evidence about this attempt, not withdrawal
        // of durable consent. Only explicit revocation paths may disable a source.
        let code = match error {
            PlatformError::Unauthorized => "unauthorized",
            PlatformError::Forbidden => "forbidden",
            PlatformError::NotFound => "not_found",
            _ => "unavailable",
        };
        AuditWriter::append(
            &mut tx,
            Some(&request.job.requested_by),
            None,
            "repository.source_access_failed",
            Some(&request.job.repository_id),
            json!({"job_id":request.job.id,"connection_id":request.connection.id,
                "connection_revoked":false,"credential_revision":request.credential_revision,"error_code":code}),
        )?;
        tx.commit().map_err(AnalysisStore::database_error)
    }
    pub fn prepare(
        &mut self,
        lease: &RepositoryJobLease,
        policy: &GitNetworkPolicy,
    ) -> Result<RepositoryJobSourcePlan, PlatformError> {
        let request = RepositoryJobSourceRequest::load(self.store, lease, policy)?;
        let (repository, commit, credentials) = match self.prove(&request, policy) {
            Ok(proof) => proof,
            Err(error) => {
                if matches!(
                    error,
                    PlatformError::Unauthorized
                        | PlatformError::Forbidden
                        | PlatformError::NotFound
                        | PlatformError::Unavailable
                ) {
                    self.reject(lease, &request, &error)?;
                }
                return Err(error);
            }
        };
        let scope = self.store.pin_repository_job(lease, &commit)?;
        let origin = match request.connection.provider {
            GitProvider::Github => policy.github_origin.clone(),
            GitProvider::Gitlab => policy.gitlab_origin.clone(),
        };
        Ok(RepositoryJobSourcePlan {
            origin,
            repository,
            commit,
            credentials,
            scope,
        })
    }
}
