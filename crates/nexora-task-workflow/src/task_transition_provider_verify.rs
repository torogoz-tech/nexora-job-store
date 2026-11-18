use super::TaskTransitionProvider;
use crate::{
    GitNetworkPolicy, TaskWorkflowStore, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn verify(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        branch: &TaskTransitionBranch,
        base: Option<&str>,
        policy: &GitNetworkPolicy,
    ) -> Result<Value, PlatformError> {
        if !DevelopmentPolicy::valid_ref(&branch.branch_name)
            || !TaskWorkBranchReport::valid_sha(&branch.expected_head_sha)
        {
            return Err(PlatformError::Validation(
                "invalid branch or expected head SHA",
            ));
        }
        let fact = Self::authority(store, ctx, project, &branch.repository_id)?;
        let (source, token) = Self::source(store, ctx, project, &branch.repository_id, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let head = Self::sha(&client, &source, &token, &branch.branch_name)?;
        if head != branch.expected_head_sha {
            return Err(PlatformError::Conflict(
                "provider branch head changed; refresh branch facts and obtain a new approval",
            ));
        }
        let base = if let Some(base) = base {
            base.to_owned()
        } else {
            let mut tx = store
                .client()
                .transaction()
                .map_err(TaskWorkflowStore::database_error)?;
            let p = crate::development_policy_store::DevelopmentPolicyStore::lock(
                &mut tx,
                project.ok_or(PlatformError::Forbidden)?,
                &branch.repository_id,
            )?;
            tx.commit().map_err(TaskWorkflowStore::database_error)?;
            p.base_branch
        };
        let base_sha = Self::revision_sha(&client, &source, &token, &base)?;
        let root = Self::root(&source)?;
        let compared = if source.connection.provider == GitProvider::Github {
            client.get(&format!("{root}/compare/{base_sha}...{head}"), &token)?
        } else {
            client.get(
                &format!("{root}/repository/merge_base?refs[]={base_sha}&refs[]={head}"),
                &token,
            )?
        };
        let ancestor = if source.connection.provider == GitProvider::Github {
            compared["merge_base_commit"]["sha"].as_str() == Some(base_sha.as_str())
        } else {
            compared["id"].as_str() == Some(base_sha.as_str())
        };
        if !ancestor {
            return Err(PlatformError::Conflict(
                "working branch does not contain the required base branch head; update and push it before retrying",
            ));
        }
        let current = PullRequestSource::load(
            store,
            ctx,
            project.ok_or(PlatformError::Forbidden)?,
            &branch.repository_id,
            policy,
        )?;
        if !source.matches(&current) {
            return Err(PlatformError::Conflict(
                "repository authority changed during branch verification",
            ));
        }
        if fact != Self::authority(store, ctx, project, &branch.repository_id)? {
            return Err(PlatformError::Conflict(
                "repository authority or development policy changed after provider verification",
            ));
        }
        store.transition_verified_facts.push(fact);
        Ok(
            json!({"repository_id":branch.repository_id,"branch_name":branch.branch_name,"head_sha":head,"base_branch":base,"base_sha":base_sha,"provenance":"provider_verified"}),
        )
    }
}
