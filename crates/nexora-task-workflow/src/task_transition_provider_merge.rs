use super::TaskTransitionProvider;
use crate::{
    TaskWorkflowStore, provider_request_context::ProviderRequestContext,
    pull_request_client::PullRequestClient, task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn verify_approval(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        task: &WorkTask,
        run: &Value,
    ) -> Result<(), PlatformError> {
        let parameters: Value = store
            .client()
            .query_one(
                "SELECT parameters FROM task_transition_runs WHERE id=$1",
                &[&run["id"].as_str().ok_or(PlatformError::Storage)?],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        let input: TaskTransitionRequest = TaskWorkflowStore::decode(parameters)?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let version = run["workflow_version_id"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let flow = crate::task_pool_workflow::TaskPoolWorkflow::flow(
            &mut tx,
            task.project_id.as_deref(),
            version,
            false,
        )?;
        let approved = crate::task_approval_store::TaskApprovalStore::consume(
            &mut tx,
            ctx,
            task,
            &flow,
            run["transition_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?,
            &input,
        )?;
        tx.rollback().map_err(TaskWorkflowStore::database_error)?;
        if !approved {
            return Err(PlatformError::Conflict("human approval is required"));
        }
        Ok(())
    }
    pub(crate) fn merge(
        context: ProviderRequestContext<'_>,
        branch: &TaskTransitionBranch,
        target: &str,
        index: usize,
        run: &mut Value,
    ) -> Result<Value, PlatformError> {
        let ProviderRequestContext {
            store,
            ctx,
            project,
            policy,
        } = context;
        use crate::task_provider_merge::TaskProviderMerge;
        if !DevelopmentPolicy::valid_ref(&branch.branch_name)
            || !DevelopmentPolicy::valid_ref(target)
            || branch.branch_name == target
            || !TaskWorkBranchReport::valid_sha(&branch.expected_head_sha)
        {
            return Err(PlatformError::Validation(
                "merge requires distinct valid branches and an exact head SHA",
            ));
        }
        let task = TaskTransitionRunStore::authorize(
            store,
            ctx,
            project,
            run["task_id"].as_str().ok_or(PlatformError::Storage)?,
        )?;
        if Some(task.revision) != run["task_revision"].as_i64() {
            return Err(PlatformError::Conflict(
                "task revision changed before merge",
            ));
        }
        let fact = Self::authority(store, ctx, project, &branch.repository_id)?;
        let (source, token) = Self::source(store, ctx, project, &branch.repository_id, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let root = Self::root(&source)?;
        let raw = TaskProviderMerge::find(
            &client,
            source.connection.provider,
            &root,
            &token,
            &branch.branch_name,
            &branch.expected_head_sha,
            target,
        )?;
        let key = format!("merge:{index}:{}:{target}", branch.repository_id);
        let result = if let Some(result) = TaskProviderMerge::confirmed(
            source.connection.provider,
            &raw,
            &branch.expected_head_sha,
            target,
        )? {
            result
        } else {
            if run["provider_intents"].get(&key).is_some() {
                return Err(PlatformError::Conflict(
                    "previous merge outcome remains unknown; reconcile the provider before another merge request",
                ));
            }
            Self::verify_approval(store, ctx, &task, run)?;
            let readiness = TaskProviderMerge::ready(
                &client,
                source.connection.provider,
                &root,
                &token,
                &raw,
                &branch.expected_head_sha,
                target,
            )?;
            let current = TaskTransitionRunStore::authorize(store, ctx, project, &task.id)?;
            if current.revision != task.revision
                || fact != Self::authority(store, ctx, project, &branch.repository_id)?
                || Self::sha(&client, &source, &token, &branch.branch_name)?
                    != branch.expected_head_sha
            {
                return Err(PlatformError::Conflict(
                    "task, repository authority or source head changed before merge",
                ));
            }
            Self::verify_approval(store, ctx, &task, run)?;
            let number = raw[if source.connection.provider == GitProvider::Github {
                "number"
            } else {
                "iid"
            }]
            .as_u64()
            .ok_or(PlatformError::Unavailable)?;
            run["provider_intents"][&key] = json!({"status":"request_started","repository_id":branch.repository_id,"head_sha":branch.expected_head_sha,"target_branch":target,"number":number,"provider_readiness":readiness});
            TaskTransitionRunStore::save(store, run)?;
            let attempted = TaskProviderMerge::write(
                &client,
                source.connection.provider,
                &root,
                &token,
                number,
                &branch.expected_head_sha,
            );
            // Never blindly retry an unknown write. A read of this exact PR is the receipt.
            let observed = TaskProviderMerge::find(
                &client,
                source.connection.provider,
                &root,
                &token,
                &branch.branch_name,
                &branch.expected_head_sha,
                target,
            )?;
            match TaskProviderMerge::confirmed(
                source.connection.provider,
                &observed,
                &branch.expected_head_sha,
                target,
            )? {
                Some(result) => result,
                None => {
                    if let Err(error) = attempted {
                        if matches!(
                            error,
                            PlatformError::Forbidden
                                | PlatformError::NotFound
                                | PlatformError::Validation(_)
                                | PlatformError::Conflict(_)
                        ) {
                            run["provider_intents"]
                                .as_object_mut()
                                .ok_or(PlatformError::Storage)?
                                .remove(&key);
                            TaskTransitionRunStore::save(store, run)?;
                        }
                        return Err(error);
                    }
                    return Err(PlatformError::Conflict(
                        "waiting for the provider to confirm the merge; its outcome must be reconciled before retrying",
                    ));
                }
            }
        };
        if fact != Self::authority(store, ctx, project, &branch.repository_id)? {
            return Err(PlatformError::Conflict(
                "repository authority changed during merge",
            ));
        }
        let mut result = result;
        result["repository_id"] = json!(branch.repository_id);
        result["source_branch"] = json!(branch.branch_name);
        run["provider_intents"][&key] = json!({"status":"confirmed","result":result});
        TaskTransitionRunStore::save(store, run)?;
        store.transition_verified_facts.push(fact);
        Ok(result)
    }
}
