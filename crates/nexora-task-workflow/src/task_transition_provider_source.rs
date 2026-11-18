use super::TaskTransitionProvider;
use crate::{
    GitNetworkPolicy, TaskWorkflowStore, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource,
};
use nexora_platform::*;
use serde_json::Value;
use zeroize::Zeroizing;

impl TaskTransitionProvider {
    pub(crate) fn encoded(value: &str) -> String {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
    pub(crate) fn authority(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        repository: &str,
    ) -> Result<Value, PlatformError> {
        let project = project.ok_or(PlatformError::Conflict(
            "Git actions require a project-linked task",
        ))?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let fact =
            crate::task_transition_provider_authority::TaskTransitionProviderAuthority::lock(
                &mut tx, ctx, project, repository,
            )?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(fact)
    }
    pub(crate) fn source(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        repository: &str,
        policy: &GitNetworkPolicy,
    ) -> Result<(PullRequestSource, Zeroizing<String>), PlatformError> {
        let project = project.ok_or(PlatformError::Conflict(
            "Git actions require a project-linked task",
        ))?;
        let source = PullRequestSource::load(store, ctx, project, repository, policy)?;
        let token = match source.connection.provider {
            GitProvider::Github => {
                let _ = crate::github_repository_proof::GithubRepositoryProof::check(
                    store,
                    &source.connection,
                    source
                        .repository
                        .remote_full_name
                        .as_deref()
                        .ok_or(PlatformError::Storage)?,
                    source.repository.remote_id.as_deref(),
                    policy,
                )?;
                let tokens = crate::github_token_store::GithubTokenStore::new(store)
                    .tokens(&source.connection, policy)?;
                Zeroizing::new(tokens.access_token.to_string())
            }
            GitProvider::Gitlab => {
                let tokens = crate::gitlab_token_store::GitlabTokenStore::new(store)
                    .tokens(&source.connection, policy)?;
                let client = crate::gitlab_client::GitlabClient::new(policy);
                let id = source
                    .repository
                    .remote_id
                    .as_deref()
                    .ok_or(PlatformError::Storage)?;
                if client.user(&tokens.access_token)?.0 != source.connection.remote_account_id
                    || client.repository(&tokens.access_token, id)?.0 != id
                {
                    return Err(PlatformError::Conflict(
                        "Git provider repository identity changed",
                    ));
                }
                Zeroizing::new(tokens.access_token.to_string())
            }
        };
        let current = PullRequestSource::load(store, ctx, project, repository, policy)?;
        if !source.matches(&current) {
            return Err(PlatformError::Conflict(
                "repository authority changed during provider verification",
            ));
        }
        Ok((current, token))
    }
    pub(crate) fn root(source: &PullRequestSource) -> Result<String, PlatformError> {
        if source.connection.provider == GitProvider::Github {
            Ok(format!(
                "/repos/{}",
                source
                    .repository
                    .remote_full_name
                    .as_deref()
                    .ok_or(PlatformError::Storage)?
            ))
        } else {
            let id = source
                .repository
                .remote_id
                .as_deref()
                .ok_or(PlatformError::Storage)?;
            if id.parse::<u64>().ok().filter(|id| *id > 0).is_none() {
                return Err(PlatformError::Storage);
            }
            Ok(format!("/api/v4/projects/{id}"))
        }
    }
    pub(crate) fn sha(
        client: &PullRequestClient,
        source: &PullRequestSource,
        token: &str,
        branch: &str,
    ) -> Result<String, PlatformError> {
        let root = Self::root(source)?;
        let path = if source.connection.provider == GitProvider::Github {
            format!("{root}/branches/{}", Self::encoded(branch))
        } else {
            format!("{root}/repository/branches/{}", Self::encoded(branch))
        };
        let value = client.get(&path, token)?;
        value["commit"][if source.connection.provider == GitProvider::Github {
            "sha"
        } else {
            "id"
        }]
        .as_str()
        .filter(|sha| TaskWorkBranchReport::valid_sha(sha))
        .map(str::to_owned)
        .ok_or(PlatformError::Unavailable)
    }
    pub(crate) fn revision_sha(
        client: &PullRequestClient,
        source: &PullRequestSource,
        token: &str,
        revision: &str,
    ) -> Result<String, PlatformError> {
        if !TaskWorkBranchReport::valid_sha(revision) {
            return Self::sha(client, source, token, revision);
        }
        let root = Self::root(source)?;
        let path = if source.connection.provider == GitProvider::Github {
            format!("{root}/commits/{revision}")
        } else {
            format!("{root}/repository/commits/{revision}")
        };
        let value = client.get(&path, token)?;
        value[if source.connection.provider == GitProvider::Github {
            "sha"
        } else {
            "id"
        }]
        .as_str()
        .filter(|sha| TaskWorkBranchReport::valid_sha(sha))
        .map(str::to_owned)
        .ok_or(PlatformError::Unavailable)
    }
}
