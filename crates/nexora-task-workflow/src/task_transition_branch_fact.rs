use crate::{
    GitNetworkPolicy, TaskWorkflowStore, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource, task_transition_provider::TaskTransitionProvider,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskWorkflowStore {
    pub fn task_transition_branch_fact(
        &mut self,
        ctx: &AuthContext,
        id: &str,
        repository: &str,
        branch: &str,
        policy: &GitNetworkPolicy,
    ) -> Result<Value, PlatformError> {
        if !DevelopmentPolicy::valid_ref(branch) {
            return Err(PlatformError::Validation("invalid branch name"));
        }
        let project = self.pool_task_scope(ctx, id, Capability::TasksRead)?;
        // Reading provider facts requires task visibility as well as the existing repository binding authority.
        self.pool_task(ctx, id)?;
        let (source, token) =
            TaskTransitionProvider::source(self, ctx, project.as_deref(), repository, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let sha = TaskTransitionProvider::sha(&client, &source, &token, branch)?;
        let current = PullRequestSource::load(
            self,
            ctx,
            project.as_deref().ok_or(PlatformError::Forbidden)?,
            repository,
            policy,
        )?;
        if !source.matches(&current) {
            return Err(PlatformError::Conflict(
                "repository authority changed during provider request",
            ));
        }
        Ok(
            json!({"repository_id":repository,"branch_name":branch,"expected_head_sha":sha,"provenance":"provider_verified"}),
        )
    }
}
