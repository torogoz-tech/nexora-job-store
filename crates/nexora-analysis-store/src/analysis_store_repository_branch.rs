use crate::{
    AnalysisStore, GitNetworkPolicy, repository_branch_head::RepositoryBranchHead,
    repository_transport_plan_store::RepositoryTransportPlanStore,
};
use nexora_platform::{GitConnection, GitProvider, PlatformError, ProjectRepository};

impl AnalysisStore {
    /// Reads a branch head after verifying the provider repository identity.
    pub fn authorized_branch_head(
        &mut self,
        connection: &GitConnection,
        repository: &ProjectRepository,
        policy: &GitNetworkPolicy,
        branch: &str,
    ) -> Result<Option<String>, PlatformError> {
        RepositoryTransportPlanStore::validate_source(connection, repository, policy)?;
        let remote_name = repository
            .remote_full_name
            .as_deref()
            .ok_or(PlatformError::Storage)?;

        match connection.provider {
            GitProvider::Github => {
                let (_, scoped) = crate::github_repository_proof::GithubRepositoryProof::check(
                    self,
                    connection,
                    remote_name,
                    repository.remote_id.as_deref(),
                    policy,
                )?;
                if scoped.full_name != remote_name {
                    return Err(PlatformError::Conflict(
                        "registered repository identity changed",
                    ));
                }
                let tokens = crate::github_token_store::GithubTokenStore::new(self)
                    .tokens(connection, policy)?;
                let path_branch =
                    url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
                match crate::github_client::GithubClient::new(policy).get(
                    &format!("/repos/{remote_name}/branches/{path_branch}"),
                    &tokens.access_token,
                ) {
                    Ok(value) => RepositoryBranchHead::from_github_response(&value).map(Some),
                    Err(PlatformError::NotFound) => Ok(None),
                    Err(error) => Err(error),
                }
            }
            GitProvider::Gitlab => {
                let tokens = crate::gitlab_token_store::GitlabTokenStore::new(self)
                    .tokens(connection, policy)?;
                let client = crate::gitlab_client::GitlabClient::new(policy);
                if client.user(&tokens.access_token)?.0 != connection.remote_account_id {
                    return Err(PlatformError::Unauthorized);
                }
                let (remote_id, name) = client.repository(&tokens.access_token, remote_name)?;
                if repository.remote_id.as_deref() != Some(remote_id.as_str())
                    || name != remote_name
                {
                    return Err(PlatformError::Conflict(
                        "registered repository identity changed",
                    ));
                }
                match client.branch(&tokens.access_token, &remote_id, branch) {
                    Ok(sha) => Ok(Some(sha)),
                    Err(PlatformError::NotFound) => Ok(None),
                    Err(error) => Err(error),
                }
            }
        }
    }
}
