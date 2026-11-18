use super::TaskTransitionProvider;
use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn review(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        branch: &TaskTransitionBranch,
        base: Option<&str>,
        policy: &GitNetworkPolicy,
        run: &mut Value,
    ) -> Result<Value, PlatformError> {
        let mut review = crate::task_pre_pr_review::TaskPrePrReview::enqueue_or_load(
            store, ctx, project, branch, base, policy, run,
        )?;
        review["repository_id"] = json!(branch.repository_id);
        Ok(review)
    }
}
