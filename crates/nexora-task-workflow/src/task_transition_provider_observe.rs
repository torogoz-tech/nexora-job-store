use super::TaskTransitionProvider;
use crate::{
    GitNetworkPolicy, TaskWorkflowStore, provider_observation_request::ProviderObservationRequest,
    provider_request_context::ProviderRequestContext, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn observe(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        branch: &TaskTransitionBranch,
        observation: &str,
        selector: Option<&str>,
        policy: &GitNetworkPolicy,
    ) -> Result<Value, PlatformError> {
        Self::observe_to(
            ProviderRequestContext {
                store,
                ctx,
                project,
                policy,
            },
            ProviderObservationRequest {
                branch,
                observation,
                selector,
                target_branch: None,
            },
        )
    }
    pub(crate) fn observe_to(
        context: ProviderRequestContext<'_>,
        request: ProviderObservationRequest<'_>,
    ) -> Result<Value, PlatformError> {
        let ProviderRequestContext {
            store,
            ctx,
            project,
            policy,
        } = context;
        let ProviderObservationRequest {
            branch,
            observation,
            selector,
            target_branch,
        } = request;
        use crate::task_provider_operations::TaskProviderOperations;
        if !DevelopmentPolicy::valid_ref(&branch.branch_name)
            || !TaskWorkBranchReport::valid_sha(&branch.expected_head_sha)
        {
            return Err(PlatformError::Validation(
                "invalid pinned provider observation",
            ));
        }
        let fact = Self::authority(store, ctx, project, &branch.repository_id)?;
        let (source, token) = Self::source(store, ctx, project, &branch.repository_id, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let root = Self::root(&source)?;
        let result = match observation {
            "pipeline" | "commit_pipeline" => {
                if observation == "pipeline"
                    && Self::sha(&client, &source, &token, &branch.branch_name)?
                        != branch.expected_head_sha
                {
                    return Err(PlatformError::Conflict(
                        "provider branch changed before pipeline verification",
                    ));
                }
                TaskProviderOperations::pipeline(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    &branch.expected_head_sha,
                    selector,
                )?
            }
            "merged_pipeline" => {
                let merged = TaskProviderOperations::merged_to(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    &branch.branch_name,
                    &branch.expected_head_sha,
                    target_branch,
                )?;
                let sha = merged["merge_sha"]
                    .as_str()
                    .ok_or(PlatformError::Unavailable)?;
                let mut checks = TaskProviderOperations::pipeline(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    sha,
                    selector,
                )?;
                checks["source_head_sha"] = json!(branch.expected_head_sha);
                checks["merge"] = merged;
                checks
            }
            "pull_request_merged" => TaskProviderOperations::merged_to(
                &client,
                source.connection.provider,
                &root,
                &token,
                &branch.branch_name,
                &branch.expected_head_sha,
                target_branch.or(selector),
            )?,
            "deployment" => {
                let environment = selector.ok_or(PlatformError::Validation(
                    "deployment environment is required",
                ))?;
                // A deployed revision is the provider-confirmed merge result, never an arbitrary client-supplied SHA.
                let merged = TaskProviderOperations::merged_to(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    &branch.branch_name,
                    &branch.expected_head_sha,
                    target_branch,
                )?;
                let sha = merged["merge_sha"]
                    .as_str()
                    .ok_or(PlatformError::Unavailable)?;
                let mut deployment = TaskProviderOperations::deployment(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    sha,
                    environment,
                )?;
                deployment["source_head_sha"] = json!(branch.expected_head_sha);
                deployment["merge"] = merged;
                deployment
            }
            _ => {
                return Err(PlatformError::Validation(
                    "unsupported provider observation",
                ));
            }
        };
        let current = PullRequestSource::load(
            store,
            ctx,
            project.ok_or(PlatformError::Forbidden)?,
            &branch.repository_id,
            policy,
        )?;
        if !source.matches(&current)
            || fact != Self::authority(store, ctx, project, &branch.repository_id)?
        {
            return Err(PlatformError::Conflict(
                "repository authority changed during provider observation",
            ));
        }
        store.transition_verified_facts.push(fact);
        let mut result = result;
        result["repository_id"] = json!(branch.repository_id);
        result["source_branch"] = json!(branch.branch_name);
        Ok(result)
    }
}
