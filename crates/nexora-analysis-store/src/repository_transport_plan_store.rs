use crate::{AnalysisStore, GitNetworkPolicy, RepositoryTransportPlan};
use nexora_platform::{
    GitConnection, GitConnectionStatus, GitProvider, PlatformError, ProjectRepository,
    RepositoryAuthorization, RepositoryRegistration, RepositorySourceKind,
};
use nexora_source::GitFetchCredentials;

pub(crate) struct RepositoryTransportPlanStore<'a> {
    store: &'a mut AnalysisStore,
}

impl<'a> RepositoryTransportPlanStore<'a> {
    pub(crate) fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }

    pub(crate) fn validate_source(
        connection: &GitConnection,
        repository: &ProjectRepository,
        policy: &GitNetworkPolicy,
    ) -> Result<(), PlatformError> {
        policy.validate()?;

        if connection.status != GitConnectionStatus::Active
            || repository.source_kind != RepositorySourceKind::Git
            || repository.authorization_status != RepositoryAuthorization::Authorized
            || repository.archived
            || repository.connection_id.as_deref() != Some(connection.id.as_str())
            || repository.provider != Some(connection.provider)
            || repository.host.as_deref() != Some(connection.host.as_str())
        {
            return Err(PlatformError::Conflict(
                "repository source is not authorized",
            ));
        }

        let policy_host = match connection.provider {
            GitProvider::Github => policy.github_host()?,
            GitProvider::Gitlab => policy.host()?,
        };
        if connection.host != policy_host {
            return Err(PlatformError::Conflict(
                "repository provider policy changed",
            ));
        }

        let remote_id = repository
            .remote_id
            .as_deref()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or(PlatformError::Storage)?;
        if remote_id.parse::<u64>().ok().filter(|id| *id > 0).is_none() {
            return Err(PlatformError::Storage);
        }
        RepositoryRegistration {
            provider: connection.provider,
            full_name: repository
                .remote_full_name
                .clone()
                .ok_or(PlatformError::Storage)?,
        }
        .locator()?;

        Ok(())
    }

    pub(crate) fn prepare(
        &mut self,
        connection: &GitConnection,
        repository: &ProjectRepository,
        policy: &GitNetworkPolicy,
        branch: &str,
    ) -> Result<RepositoryTransportPlan, PlatformError> {
        Self::validate_source(connection, repository, policy)?;
        let remote_name = repository
            .remote_full_name
            .as_deref()
            .ok_or(PlatformError::Storage)?;

        let credentials = match connection.provider {
            GitProvider::Github => {
                let (_, scoped) = crate::github_repository_proof::GithubRepositoryProof::check(
                    self.store,
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
                let tokens = crate::github_token_store::GithubTokenStore::new(self.store)
                    .tokens(connection, policy)?;
                let token = tokens.access_token.to_string();
                let branch =
                    url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
                let value = crate::github_client::GithubClient::new(policy)
                    .get(&format!("/repos/{remote_name}/branches/{branch}"), &token)?;
                let base_commit = value["commit"]["sha"]
                    .as_str()
                    .filter(|sha| crate::git_commit_identity::GitCommitIdentity::valid(sha))
                    .ok_or(PlatformError::Unavailable)?
                    .to_owned();
                let credentials = GitFetchCredentials::new("x-access-token", token)
                    .map_err(|_| PlatformError::Unavailable)?;
                (credentials, base_commit)
            }
            GitProvider::Gitlab => {
                let mut tokens = crate::gitlab_token_store::GitlabTokenStore::new(self.store)
                    .tokens(connection, policy)?;
                let client = crate::gitlab_client::GitlabClient::new(policy);
                if client.user(&tokens.access_token)?.0 != connection.remote_account_id {
                    return Err(PlatformError::Unauthorized);
                }
                let (id, name) = client.repository(&tokens.access_token, remote_name)?;
                if id
                    != repository
                        .remote_id
                        .as_deref()
                        .ok_or(PlatformError::Storage)?
                    || name != remote_name
                {
                    return Err(PlatformError::Conflict(
                        "registered repository identity changed",
                    ));
                }
                let base_commit = client.branch(&tokens.access_token, &id, branch)?;
                let credentials =
                    GitFetchCredentials::new("oauth2", std::mem::take(&mut tokens.access_token))
                        .map_err(|_| PlatformError::Unavailable)?;
                (credentials, base_commit)
            }
        };

        let origin = match connection.provider {
            GitProvider::Github => policy.github_origin.clone(),
            GitProvider::Gitlab => policy.gitlab_origin.clone(),
        };
        Ok(RepositoryTransportPlan::new(
            origin,
            remote_name.into(),
            credentials.1,
            credentials.0,
        ))
    }
}
