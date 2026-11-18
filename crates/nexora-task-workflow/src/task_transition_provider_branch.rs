use super::TaskTransitionProvider;
use crate::{
    provider_execution_context::ProviderExecutionContext, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource, task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn create_branch(
        execution: &mut ProviderExecutionContext<'_>,
        branch: &TaskTransitionBranch,
        base_branch: &str,
        expected_base_sha: &str,
        index: usize,
    ) -> Result<Value, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let policy = execution.policy;
        let run = &mut *execution.run;
        use crate::task_provider_operations::TaskProviderOperations;
        if !DevelopmentPolicy::valid_ref(&branch.branch_name)
            || !DevelopmentPolicy::valid_ref(base_branch)
            || !TaskWorkBranchReport::valid_sha(expected_base_sha)
            || branch.expected_head_sha != expected_base_sha
            || branch.branch_name == base_branch
        {
            return Err(PlatformError::Validation(
                "branch creation requires a distinct branch and exact pinned base SHA",
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
                "task revision changed before branch creation",
            ));
        }
        let fact = Self::authority(store, ctx, project, &branch.repository_id)?;
        let (source, token) = Self::source(store, ctx, project, &branch.repository_id, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let root = Self::root(&source)?;
        let actual_base = if TaskWorkBranchReport::valid_sha(base_branch) {
            let path = if source.connection.provider == GitProvider::Github {
                format!("{root}/commits/{base_branch}")
            } else {
                format!("{root}/repository/commits/{base_branch}")
            };
            let commit = client.get(&path, &token)?;
            commit[if source.connection.provider == GitProvider::Github {
                "sha"
            } else {
                "id"
            }]
            .as_str()
            .filter(|sha| TaskWorkBranchReport::valid_sha(sha))
            .ok_or(PlatformError::Unavailable)?
            .to_owned()
        } else {
            Self::sha(&client, &source, &token, base_branch)?
        };
        let key = format!(
            "branch:{index}:{}:{}",
            branch.repository_id, branch.branch_name
        );
        let existing = Self::sha(&client, &source, &token, &branch.branch_name);
        if actual_base != expected_base_sha
            && !(run["provider_intents"].get(&key).is_some()
                && existing.as_ref().is_ok_and(|sha| sha == expected_base_sha))
        {
            return Err(PlatformError::Conflict(
                "provider base branch changed; refresh pinned branch creation facts",
            ));
        }
        match existing {
            Ok(sha) if sha == expected_base_sha => {}
            Ok(_) => {
                return Err(PlatformError::Conflict(
                    "existing branch has a different SHA; automatic overwrite is forbidden",
                ));
            }
            Err(PlatformError::NotFound) => {
                if run["provider_intents"].get(&key).is_some() {
                    return Err(PlatformError::Conflict(
                        "previous branch creation outcome remains unknown; reconcile it at the provider before retrying",
                    ));
                }
                let current_task =
                    TaskTransitionRunStore::authorize(store, ctx, project, &task.id)?;
                if current_task.revision != task.revision
                    || fact != Self::authority(store, ctx, project, &branch.repository_id)?
                {
                    return Err(PlatformError::Conflict(
                        "task or repository authority changed before branch creation",
                    ));
                }
                run["provider_intents"][&key] = json!({"status":"request_started","repository_id":branch.repository_id,"branch_name":branch.branch_name,"base_branch":base_branch,"base_sha":expected_base_sha});
                TaskTransitionRunStore::save(store, run)?;
                let result = TaskProviderOperations::create_branch(
                    &client,
                    source.connection.provider,
                    &root,
                    &token,
                    &branch.branch_name,
                    expected_base_sha,
                );
                // Reconcile even explicit rejection: another request may have created the same exact ref.
                match result {
                    Ok(()) => {}
                    Err(error) => match Self::sha(&client, &source, &token, &branch.branch_name) {
                        Ok(sha) if sha == expected_base_sha => {}
                        _ => return Err(error),
                    },
                }
            }
            Err(error) => return Err(error),
        }
        if Self::sha(&client, &source, &token, &branch.branch_name)? != expected_base_sha {
            return Err(PlatformError::Conflict(
                "provider branch changed after creation",
            ));
        }
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
                "repository authority changed during branch creation",
            ));
        }
        let result = json!({"repository_id":branch.repository_id,"branch_name":branch.branch_name,"head_sha":expected_base_sha,"base_branch":base_branch,"base_sha":expected_base_sha,"provenance":"provider_verified"});
        run["provider_intents"][&key] = json!({"status":"confirmed","result":result});
        TaskTransitionRunStore::save(store, run)?;
        store.transition_verified_facts.push(fact);
        Ok(result)
    }
}
