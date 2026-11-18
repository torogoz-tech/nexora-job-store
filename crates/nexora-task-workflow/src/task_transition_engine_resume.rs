use super::TaskTransitionEngine;
use crate::{
    TaskWorkflowStore, task_transition_execution_context::TaskTransitionExecutionContext,
    task_transition_provider::TaskTransitionProvider,
    task_transition_run_store::TaskTransitionRunStore, task_transition_store::TaskTransitionStore,
};
use nexora_platform::*;
use serde_json::json;

impl TaskTransitionEngine {
    pub(crate) fn resume(
        execution: &mut TaskTransitionExecutionContext<'_>,
    ) -> Result<WorkTask, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let id = execution.task_id;
        let transition = execution.transition_id;
        let input = execution.input;
        let policy = execution.policy;
        let run = &mut *execution.run;
        let task = TaskTransitionRunStore::authorize(store, ctx, project, id)?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let version = run["workflow_version_id"]
            .as_str()
            .or(task.workflow_version_id.as_deref())
            .ok_or(PlatformError::Conflict(
                "task must be taken and resumed before transitioning",
            ))?;
        let flow =
            crate::task_pool_workflow::TaskPoolWorkflow::flow(&mut tx, project, version, false)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        flow.graph.validate()?;
        let edge = flow
            .graph
            .transitions
            .iter()
            .find(|edge| edge.id == transition)
            .ok_or(PlatformError::Conflict(
                "transition is outside pinned workflow",
            ))?
            .clone();
        run["workflow_version_id"] = json!(flow.id);
        let steps = if edge.steps.is_empty() {
            vec![WorkflowTransitionStep::AdvanceState]
        } else {
            edge.steps.clone()
        };
        if steps
            .iter()
            .any(|step| matches!(step, WorkflowTransitionStep::PublishRelease))
        {
            crate::task_release_publication::TaskReleasePublication::selection(
                store, ctx, project, input, run,
            )?;
        } else if input.release_id.is_some() || input.release_revision.is_some() {
            return Err(PlatformError::Validation(
                "release parameters require a publish_release final pipeline",
            ));
        }
        let required_base = flow
            .graph
            .states
            .iter()
            .find(|state| state.id == edge.to)
            .and_then(|state| state.required_base_branch.as_deref())
            .or_else(|| {
                flow.graph
                    .states
                    .iter()
                    .find(|state| state.id == edge.from)
                    .and_then(|state| state.required_base_branch.as_deref())
            });
        if run["advanced"] != true {
            Self::preflight(store, ctx, project, id, transition, input, &flow)?;
            if flow
                .graph
                .states
                .iter()
                .any(|state| state.id == edge.to && state.terminal)
            {
                crate::task_subtask_integration::TaskSubtaskIntegration::verify(
                    store, ctx, &task, policy,
                )?;
            }
            if let Some(base) = required_base {
                if input.branches.is_empty() {
                    return Err(PlatformError::Conflict(
                        "state requires repository branches verified from its required base branch",
                    ));
                }
                for branch in &input.branches {
                    let created_here=steps.iter().any(|step|matches!(step,WorkflowTransitionStep::CreateBranch{repository_id,..} if repository_id.as_deref().is_none_or(|r|r==branch.repository_id)));
                    if !created_here {
                        Self::verify_branch(
                            &mut TaskTransitionExecutionContext {
                                store: &mut *store,
                                ctx,
                                project,
                                task_id: id,
                                transition_id: transition,
                                input,
                                policy,
                                run: &mut *run,
                            },
                            branch,
                            Some(base),
                        )?;
                    }
                }
            }
        }
        let start = run["next_step"].as_u64().unwrap_or(0) as usize;
        for (index, step) in steps.iter().enumerate().skip(start) {
            TaskTransitionRunStore::authorize(store, ctx, project, id)?;
            let outcome = match step {
                WorkflowTransitionStep::RequireApproval => {
                    TaskTransitionProvider::verify_approval(store, ctx, &task, run)?;
                    json!({"approval_id":input.approval_id})
                }
                WorkflowTransitionStep::VerifyPush { repository_id } => {
                    let branches = Self::branches(input, repository_id.as_deref())?;
                    let mut results = Vec::new();
                    for branch in branches {
                        results.push(Self::verify_branch(
                            &mut TaskTransitionExecutionContext {
                                store: &mut *store,
                                ctx,
                                project,
                                task_id: id,
                                transition_id: transition,
                                input,
                                policy,
                                run: &mut *run,
                            },
                            branch,
                            None,
                        )?);
                    }
                    json!({"branches":results})
                }
                WorkflowTransitionStep::CreatePullRequest {
                    repository_id,
                    source_branch,
                    target_branch,
                    source_from_task,
                    target_from_parent,
                } => {
                    let mut execution =
                        crate::provider_execution_context::ProviderExecutionContext {
                            store,
                            ctx,
                            project,
                            policy,
                            run,
                        };
                    let results = Self::create_requests(
                        &mut execution,
                        &task,
                        input,
                        crate::pull_request_creation_step::PullRequestCreationStep {
                            repository: repository_id.as_deref(),
                            literal_source: source_branch,
                            literal_target: target_branch,
                            from_task: *source_from_task,
                            from_parent: *target_from_parent,
                            index,
                            revalidate: false,
                        },
                    )?;
                    json!({"pull_requests":results})
                }
                WorkflowTransitionStep::MergePullRequest {
                    repository_id,
                    target_branch,
                    target_from_parent,
                } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let target = crate::task_branch_selectors::TaskBranchSelectors::target(
                            store,
                            &task,
                            &branch.repository_id,
                            Some(target_branch),
                            *target_from_parent,
                        )?
                        .ok_or(PlatformError::Storage)?;
                        results.push(
                            crate::task_transition_provider::TaskTransitionProvider::merge(
                                crate::provider_request_context::ProviderRequestContext {
                                    store,
                                    ctx,
                                    project,
                                    policy,
                                },
                                branch,
                                &target,
                                index,
                                run,
                            )?,
                        );
                    }
                    json!({"pull_requests":results})
                }
                WorkflowTransitionStep::GenerateContributorRatings { repository_id } => {
                    let mut generated = 0u64;
                    let mut unrated = 0u64;
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let result =
                            crate::task_contributor_rating::TaskContributorRating::enqueue(
                                store,
                                ctx,
                                project,
                                branch,
                                required_base,
                                policy,
                                run,
                            )?;
                        generated += result["generated_count"].as_u64().unwrap_or(0);
                        unrated += result["unrated_count"].as_u64().unwrap_or(0);
                    }
                    json!({"status":"completed","generated_count":generated,"unrated_count":unrated})
                }
                WorkflowTransitionStep::CreateCodeReview { repository_id } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let upcoming_target =
                            steps.iter().skip(index + 1).find_map(|step| match step {
                                WorkflowTransitionStep::CreatePullRequest {
                                    repository_id,
                                    target_branch,
                                    target_from_parent,
                                    ..
                                } if repository_id
                                    .as_deref()
                                    .is_none_or(|r| r == branch.repository_id) =>
                                {
                                    Some((target_branch, *target_from_parent))
                                }
                                _ => None,
                            });
                        let review_base = if let Some((literal, parent)) = upcoming_target {
                            crate::task_branch_selectors::TaskBranchSelectors::target(
                                store,
                                &task,
                                &branch.repository_id,
                                Some(literal),
                                parent,
                            )?
                        } else {
                            required_base.map(str::to_owned)
                        };
                        results.push(TaskTransitionProvider::review(
                            store,
                            ctx,
                            project,
                            branch,
                            review_base.as_deref(),
                            policy,
                            run,
                        )?);
                    }
                    json!({"reviews":results})
                }
                WorkflowTransitionStep::CreateBranch {
                    repository_id,
                    base_branch,
                } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let (base, mut sha) = crate::task_branch_origin::TaskBranchOrigin::resolve(
                            store,
                            id,
                            &branch.repository_id,
                            base_branch,
                            &branch.expected_head_sha,
                        )?;
                        if !TaskWorkBranchReport::valid_sha(&base) {
                            sha = store.task_transition_branch_fact(
                                ctx,
                                id,
                                &branch.repository_id,
                                &base,
                                policy,
                            )?["expected_head_sha"]
                                .as_str()
                                .ok_or(PlatformError::Unavailable)?
                                .to_owned();
                        }
                        if branch.expected_head_sha != sha {
                            return Err(PlatformError::Conflict(
                                "expected branch creation SHA differs from the parent or tested revision; refresh provider facts before retrying",
                            ));
                        }
                        let prepared = branch.clone();
                        let mut execution =
                            crate::provider_execution_context::ProviderExecutionContext {
                                store,
                                ctx,
                                project,
                                policy,
                                run,
                            };
                        let evidence = TaskTransitionProvider::create_branch(
                            &mut execution,
                            &prepared,
                            &base,
                            &sha,
                            index,
                        )?;
                        crate::task_branch_origin::TaskBranchOrigin::save(
                            store, id, &prepared, &base, &sha, &evidence,
                        )?;
                        if !run["branch_bases"].is_object() {
                            run["branch_bases"] = json!({});
                        }
                        run["branch_bases"]
                            [format!("{}:{}", prepared.repository_id, prepared.branch_name)] =
                            json!(sha);
                        results.push(evidence);
                    }
                    json!({"branches":results})
                }
                WorkflowTransitionStep::VerifyPipeline {
                    repository_id,
                    workflow,
                } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        results.push(Self::pipeline(
                            &mut TaskTransitionExecutionContext {
                                store: &mut *store,
                                ctx,
                                project,
                                task_id: id,
                                transition_id: transition,
                                input,
                                policy,
                                run: &mut *run,
                            },
                            branch,
                            workflow.as_deref(),
                            index,
                        )?);
                    }
                    json!({"checks":results})
                }
                WorkflowTransitionStep::VerifyPullRequestMerged {
                    repository_id,
                    target_branch,
                    target_from_parent,
                } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let target = crate::task_branch_selectors::TaskBranchSelectors::target(
                            store,
                            &task,
                            &branch.repository_id,
                            target_branch.as_deref(),
                            *target_from_parent,
                        )?;
                        results.push(
                            crate::task_transition_provider::TaskTransitionProvider::observe_to(
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
                                    target_branch: target.as_deref(),
                                },
                            )?,
                        );
                    }
                    json!({"pull_requests":results})
                }
                WorkflowTransitionStep::VerifyDeployment {
                    repository_id,
                    environment,
                    target_branch,
                    target_from_parent,
                } => {
                    let mut results = Vec::new();
                    for branch in Self::branches(input, repository_id.as_deref())? {
                        let target = crate::task_branch_selectors::TaskBranchSelectors::target(
                            store,
                            &task,
                            &branch.repository_id,
                            target_branch.as_deref(),
                            *target_from_parent,
                        )?;
                        results.push(
                            crate::task_transition_provider::TaskTransitionProvider::observe_to(
                                crate::provider_request_context::ProviderRequestContext {
                                    store,
                                    ctx,
                                    project,
                                    policy,
                                },
                                crate::provider_observation_request::ProviderObservationRequest {
                                    branch,
                                    observation: "deployment",
                                    selector: Some(environment),
                                    target_branch: target.as_deref(),
                                },
                            )?,
                        );
                    }
                    json!({"deployments":results})
                }
                WorkflowTransitionStep::PublishRelease => {
                    let publication =
                        crate::task_release_publication::TaskReleasePublication::execute(
                            &mut TaskTransitionExecutionContext {
                                store: &mut *store,
                                ctx,
                                project,
                                task_id: id,
                                transition_id: transition,
                                input,
                                policy,
                                run: &mut *run,
                            },
                        )?;
                    run["result"] = serde_json::to_value(store.pool_task(ctx, id)?)
                        .map_err(|_| PlatformError::Storage)?;
                    publication
                }
                WorkflowTransitionStep::Handoff { workflow_id } => {
                    crate::task_handoff_provider_evidence::TaskHandoffProviderEvidence::refresh(
                        store,
                        ctx,
                        project,
                        input,
                        policy,
                        &run["outcomes"],
                    )?;
                    let result = crate::task_stage_store::TaskStageStore::handoff(
                        &mut TaskTransitionExecutionContext {
                            store: &mut *store,
                            ctx,
                            project,
                            task_id: id,
                            transition_id: transition,
                            input,
                            policy,
                            run: &mut *run,
                        },
                        workflow_id,
                        index,
                    )?;
                    run["result"] = json!(result);
                    json!({"workflow_id":workflow_id,"task_revision":result.revision})
                }
                WorkflowTransitionStep::AdvanceState => {
                    // Re-read all provider facts immediately before the atomic state transaction.
                    for prior in steps.iter().take(index) {
                        match prior {
                            WorkflowTransitionStep::VerifyPush { repository_id } => {
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    Self::verify_branch(
                                        &mut TaskTransitionExecutionContext {
                                            store: &mut *store,
                                            ctx,
                                            project,
                                            task_id: id,
                                            transition_id: transition,
                                            input,
                                            policy,
                                            run: &mut *run,
                                        },
                                        branch,
                                        None,
                                    )?;
                                }
                            }
                            WorkflowTransitionStep::CreatePullRequest {
                                repository_id,
                                source_branch,
                                target_branch,
                                source_from_task,
                                target_from_parent,
                            } => {
                                let prior_index = steps
                                    .iter()
                                    .position(|step| std::ptr::eq(step, prior))
                                    .ok_or(PlatformError::Storage)?;
                                let mut execution =
                                    crate::provider_execution_context::ProviderExecutionContext {
                                        store,
                                        ctx,
                                        project,
                                        policy,
                                        run,
                                    };
                                Self::create_requests(
                                    &mut execution,
                                    &task,
                                    input,
                                    crate::pull_request_creation_step::PullRequestCreationStep {
                                        repository: repository_id.as_deref(),
                                        literal_source: source_branch,
                                        literal_target: target_branch,
                                        from_task: *source_from_task,
                                        from_parent: *target_from_parent,
                                        index: prior_index,
                                        revalidate: true,
                                    },
                                )?;
                            }
                            WorkflowTransitionStep::VerifyPipeline {
                                repository_id,
                                workflow,
                            } => {
                                let prior_index = steps
                                    .iter()
                                    .position(|step| std::ptr::eq(step, prior))
                                    .ok_or(PlatformError::Storage)?;
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    Self::pipeline(
                                        &mut TaskTransitionExecutionContext {
                                            store: &mut *store,
                                            ctx,
                                            project,
                                            task_id: id,
                                            transition_id: transition,
                                            input,
                                            policy,
                                            run: &mut *run,
                                        },
                                        branch,
                                        workflow.as_deref(),
                                        prior_index,
                                    )?;
                                }
                            }
                            WorkflowTransitionStep::VerifyPullRequestMerged {
                                repository_id,
                                target_branch,
                                target_from_parent,
                            } => {
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    let target =
                                        crate::task_branch_selectors::TaskBranchSelectors::target(
                                            store,
                                            &task,
                                            &branch.repository_id,
                                            target_branch.as_deref(),
                                            *target_from_parent,
                                        )?;
                                    crate::task_transition_provider::TaskTransitionProvider::observe_to(
    crate::provider_request_context::ProviderRequestContext { store, ctx, project, policy },
    crate::provider_observation_request::ProviderObservationRequest { branch, observation: "pull_request_merged", selector: None, target_branch: target.as_deref() },
)?;
                                }
                            }
                            WorkflowTransitionStep::MergePullRequest {
                                repository_id,
                                target_branch,
                                target_from_parent,
                            } => {
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    let target =
                                        crate::task_branch_selectors::TaskBranchSelectors::target(
                                            store,
                                            &task,
                                            &branch.repository_id,
                                            Some(target_branch),
                                            *target_from_parent,
                                        )?;
                                    crate::task_transition_provider::TaskTransitionProvider::observe_to(
    crate::provider_request_context::ProviderRequestContext { store, ctx, project, policy },
    crate::provider_observation_request::ProviderObservationRequest { branch, observation: "pull_request_merged", selector: None, target_branch: target.as_deref() },
)?;
                                }
                            }
                            WorkflowTransitionStep::VerifyDeployment {
                                repository_id,
                                environment,
                                target_branch,
                                target_from_parent,
                            } => {
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    let target =
                                        crate::task_branch_selectors::TaskBranchSelectors::target(
                                            store,
                                            &task,
                                            &branch.repository_id,
                                            target_branch.as_deref(),
                                            *target_from_parent,
                                        )?;
                                    crate::task_transition_provider::TaskTransitionProvider::observe_to(
    crate::provider_request_context::ProviderRequestContext { store, ctx, project, policy },
    crate::provider_observation_request::ProviderObservationRequest { branch, observation: "deployment", selector: Some(environment), target_branch: target.as_deref() },
)?;
                                }
                            }
                            WorkflowTransitionStep::GenerateContributorRatings {
                                repository_id,
                            } => {
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    crate::task_contributor_rating::TaskContributorRating::context(
                                        store,
                                        id,
                                        task.revision,
                                        &branch.repository_id,
                                        &branch.expected_head_sha,
                                    )?;
                                }
                            }
                            WorkflowTransitionStep::CreateCodeReview { repository_id } => {
                                let prior_index = steps
                                    .iter()
                                    .position(|step| std::ptr::eq(step, prior))
                                    .ok_or(PlatformError::Storage)?;
                                for branch in Self::branches(input, repository_id.as_deref())? {
                                    let review = run["outcomes"]
                                        .as_array()
                                        .and_then(|outcomes| {
                                            outcomes.iter().find(|o| {
                                                o["step_index"].as_u64() == Some(prior_index as u64)
                                            })
                                        })
                                        .and_then(|o| o["result"]["reviews"].as_array())
                                        .and_then(|reviews| {
                                            reviews.iter().find(|r| {
                                                r["repository_id"] == branch.repository_id
                                            })
                                        })
                                        .ok_or(PlatformError::Storage)?;
                                    crate::task_pre_pr_review::TaskPrePrReview::context(
                                        store,
                                        ctx,
                                        &crate::task_review_locator::TaskReviewLocator {
                                            project: project.ok_or(PlatformError::Forbidden)?,
                                            task: id,
                                            revision: task.revision,
                                            repository: &branch.repository_id,
                                            head: &branch.expected_head_sha,
                                            review_id: Some(
                                                review["id"]
                                                    .as_str()
                                                    .ok_or(PlatformError::Storage)?,
                                            ),
                                        },
                                    )?;
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(base) = required_base {
                        for branch in Self::branches(input, None)? {
                            Self::verify_branch(
                                &mut TaskTransitionExecutionContext {
                                    store: &mut *store,
                                    ctx,
                                    project,
                                    task_id: id,
                                    transition_id: transition,
                                    input,
                                    policy,
                                    run: &mut *run,
                                },
                                branch,
                                Some(base),
                            )?;
                        }
                    }
                    if flow
                        .graph
                        .states
                        .iter()
                        .any(|state| state.id == edge.to && state.terminal)
                    {
                        crate::task_subtask_integration::TaskSubtaskIntegration::verify(
                            store, ctx, &task, policy,
                        )?;
                    }
                    // Advance and durable run checkpoint share one transaction via the core transition store.
                    store.transition_run_checkpoint = Some((
                        run["id"].as_str().ok_or(PlatformError::Storage)?.to_owned(),
                        index + 1,
                    ));
                    let outer = store.request_context.take();
                    let result =
                        TaskTransitionStore::advance(store, ctx, project, id, transition, input);
                    store.request_context = outer;
                    store.transition_run_checkpoint = None;
                    let task = result?;
                    run["advanced"] = json!(true);
                    run["result"] = json!(task);
                    json!({"task_revision":task.revision})
                }
                WorkflowTransitionStep::Notify { channel, audience } => Self::notify(
                    store,
                    ctx,
                    id,
                    index,
                    channel,
                    audience.as_deref().unwrap_or("assignee"),
                    run,
                )?,
            };
            run["outcomes"].as_array_mut().ok_or(PlatformError::Storage)?.push(json!({"step_index":index,"kind":serde_json::to_value(step).map_err(|_|PlatformError::Storage)?["kind"],"result":outcome}));
            run["next_step"] = json!(index + 1);
            TaskTransitionRunStore::save(store, run)?;
        }
        run["status"] = json!("succeeded");
        TaskTransitionRunStore::save(store, run)?;
        TaskWorkflowStore::decode(run["result"].clone())
    }
}
