use super::TaskTransitionEngine;
use crate::{
    TaskWorkflowStore, provider_execution_context::ProviderExecutionContext,
    pull_request_creation_step::PullRequestCreationStep,
    task_transition_execution_context::TaskTransitionExecutionContext,
    task_transition_provider::TaskTransitionProvider,
    task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionEngine {
    pub(crate) fn create_requests(
        execution: &mut ProviderExecutionContext<'_>,
        task: &WorkTask,
        input: &TaskTransitionRequest,
        step: PullRequestCreationStep<'_>,
    ) -> Result<Vec<Value>, PlatformError> {
        let ProviderExecutionContext {
            store,
            ctx,
            project,
            policy,
            run,
        } = execution;
        let store = &mut **store;
        let run = &mut **run;
        let mut results = Vec::new();
        for branch in Self::branches(input, step.repository)? {
            let Some(source) = crate::task_branch_selectors::TaskBranchSelectors::source(
                store,
                task,
                branch,
                step.literal_source,
                step.from_task,
                input.release_id.is_some(),
            )?
            else {
                continue;
            };
            let target = crate::task_branch_selectors::TaskBranchSelectors::target(
                store,
                task,
                &branch.repository_id,
                Some(step.literal_target),
                step.from_parent,
            )?
            .ok_or(PlatformError::Storage)?;
            if step.revalidate && Self::merged_branch(run, branch, Some(&target)).is_some() {
                results.push(
                    crate::task_transition_provider::TaskTransitionProvider::observe_to(
                        crate::provider_request_context::ProviderRequestContext {
                            store,
                            ctx,
                            project: *project,
                            policy,
                        },
                        crate::provider_observation_request::ProviderObservationRequest {
                            branch,
                            observation: "pull_request_merged",
                            selector: None,
                            target_branch: Some(&target),
                        },
                    )?,
                );
            } else {
                results.push(TaskTransitionProvider::create(
                    &mut ProviderExecutionContext {
                        store,
                        ctx,
                        project: *project,
                        policy,
                        run,
                    },
                    branch,
                    &source,
                    &target,
                    &task.title,
                    step.index,
                )?);
            }
        }
        if results.is_empty() {
            return Err(PlatformError::Conflict(
                "pull request source requires an exact branch fact matching its configured literal or registered task branch",
            ));
        }
        Ok(results)
    }
    pub(crate) fn merged_branch(
        run: &Value,
        branch: &TaskTransitionBranch,
        target: Option<&str>,
    ) -> Option<Value> {
        run["outcomes"]
            .as_array()?
            .iter()
            .rev()
            .filter(|step| {
                step["kind"] == "merge_pull_request" || step["kind"] == "verify_pull_request_merged"
            })
            .filter_map(|step| step["result"]["pull_requests"].as_array())
            .flatten()
            .find(|fact| {
                fact["repository_id"] == branch.repository_id
                    && fact["head_sha"] == branch.expected_head_sha
                    && fact["status"] == "merged"
                    && target.is_none_or(|t| fact["target_branch"] == t)
            })
            .cloned()
    }
    pub(crate) fn merged_before(
        run: &Value,
        branch: &TaskTransitionBranch,
        index: usize,
    ) -> Option<Value> {
        let outcomes: Vec<_> = run["outcomes"]
            .as_array()?
            .iter()
            .filter(|step| {
                step["step_index"]
                    .as_u64()
                    .is_some_and(|i| i < index as u64)
            })
            .cloned()
            .collect();
        Self::merged_branch(&json!({"outcomes":outcomes}), branch, None)
    }
    pub(crate) fn pipeline(
        execution: &mut TaskTransitionExecutionContext<'_>,
        branch: &TaskTransitionBranch,
        workflow: Option<&str>,
        index: usize,
    ) -> Result<Value, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let policy = execution.policy;
        let run = &*execution.run;
        if let Some(merged) = Self::merged_before(run, branch, index) {
            crate::task_transition_provider::TaskTransitionProvider::observe_to(
                crate::provider_request_context::ProviderRequestContext {
                    store,
                    ctx,
                    project,
                    policy,
                },
                crate::provider_observation_request::ProviderObservationRequest {
                    branch,
                    observation: "merged_pipeline",
                    selector: workflow,
                    target_branch: merged["target_branch"].as_str(),
                },
            )
        } else {
            let kind = if Self::merged_branch(run, branch, None).is_some() {
                "commit_pipeline"
            } else {
                "pipeline"
            };
            TaskTransitionProvider::observe(store, ctx, project, branch, kind, workflow, policy)
        }
    }
    pub(crate) fn verify_branch(
        execution: &mut TaskTransitionExecutionContext<'_>,
        branch: &TaskTransitionBranch,
        base: Option<&str>,
    ) -> Result<Value, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let task = execution.task_id;
        let policy = execution.policy;
        let run = &mut *execution.run;
        if let Some(merged) = Self::merged_branch(run, branch, None) {
            return crate::task_transition_provider::TaskTransitionProvider::observe_to(
                crate::provider_request_context::ProviderRequestContext {
                    store,
                    ctx,
                    project,
                    policy,
                },
                crate::provider_observation_request::ProviderObservationRequest {
                    branch,
                    observation: "pull_request_merged",
                    selector: None,
                    target_branch: merged["target_branch"].as_str(),
                },
            );
        }
        let key = format!("{}:{}", branch.repository_id, branch.branch_name);
        let pinned = run["branch_bases"][&key].as_str().map(str::to_owned);
        let origin = if pinned.is_none() {
            store.client().query_opt("SELECT b.base_sha FROM task_repository_branches b JOIN task_subtasks s ON s.child_task_id=b.task_id AND s.removed_at IS NULL WHERE b.task_id=$1 AND b.repository_id=$2 AND b.branch_name=$3",&[&task,&branch.repository_id,&branch.branch_name]).map_err(TaskWorkflowStore::database_error)?.map(|row|row.get::<_,String>(0))
        } else {
            None
        };
        let result = TaskTransitionProvider::verify(
            store,
            ctx,
            project,
            branch,
            pinned.as_deref().or(origin.as_deref()).or(base),
            policy,
        )?;
        if !run["branch_bases"].is_object() {
            run["branch_bases"] = json!({});
        }
        run["branch_bases"][&key] = result["base_sha"].clone();
        let mut registered_evidence = result.clone();
        registered_evidence["workflow_version_id"] = run["workflow_version_id"].clone();
        registered_evidence["task_revision"] = run["task_revision"].clone();
        store.client().execute("UPDATE task_repository_branches SET head_sha=$4,evidence=$5 WHERE task_id=$1 AND repository_id=$2 AND branch_name=$3",&[&task,&branch.repository_id,&branch.branch_name,&branch.expected_head_sha,&registered_evidence]).map_err(TaskWorkflowStore::database_error)?;
        TaskTransitionRunStore::save(store, run)?;
        Ok(result)
    }
    pub(crate) fn branches<'a>(
        input: &'a TaskTransitionRequest,
        repository: Option<&str>,
    ) -> Result<Vec<&'a TaskTransitionBranch>, PlatformError> {
        if input.branches.is_empty() || input.branches.len() > 8 {
            return Err(PlatformError::Conflict(
                "provide 1..8 repository branch facts including expected_head_sha",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for branch in &input.branches {
            if !ids.insert((&branch.repository_id, &branch.branch_name))
                || !DevelopmentPolicy::valid_ref(&branch.branch_name)
                || !TaskWorkBranchReport::valid_sha(&branch.expected_head_sha)
            {
                return Err(PlatformError::Validation(
                    "invalid or duplicate transition branch facts",
                ));
            }
        }
        let branches: Vec<_> = input
            .branches
            .iter()
            .filter(|branch| repository.is_none_or(|id| id == branch.repository_id))
            .collect();
        if branches.is_empty() {
            return Err(PlatformError::Conflict(
                "transition step repository requires an expected branch fact",
            ));
        }
        Ok(branches)
    }
}
