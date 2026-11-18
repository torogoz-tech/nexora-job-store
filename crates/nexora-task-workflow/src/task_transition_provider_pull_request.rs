use super::TaskTransitionProvider;
use crate::{
    provider_execution_context::ProviderExecutionContext, pull_request_client::PullRequestClient,
    pull_request_source::PullRequestSource, task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionProvider {
    pub(crate) fn create(
        execution: &mut ProviderExecutionContext<'_>,
        branch: &TaskTransitionBranch,
        source_branch: &str,
        target_branch: &str,
        title: &str,
        index: usize,
    ) -> Result<Value, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let policy = execution.policy;
        let run = &mut *execution.run;
        // Pull request creation requires both refs to exist; branch creation is a separate durable step.
        let task = TaskTransitionRunStore::authorize(
            store,
            ctx,
            project,
            run["task_id"].as_str().ok_or(PlatformError::Storage)?,
        )?;
        if task.revision
            != run["task_revision"]
                .as_i64()
                .ok_or(PlatformError::Storage)?
        {
            return Err(PlatformError::Conflict(
                "task revision changed before provider write",
            ));
        }
        let fact = Self::authority(store, ctx, project, &branch.repository_id)?;
        let (source, token) = Self::source(store, ctx, project, &branch.repository_id, policy)?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let root = Self::root(&source)?;
        let head = Self::sha(&client, &source, &token, source_branch)?;
        if head != branch.expected_head_sha {
            return Err(PlatformError::Conflict(
                "provider branch head changed before pull request creation",
            ));
        }
        let target_sha = Self::sha(&client, &source, &token, target_branch)?;
        let review_description = if run["outcomes"].as_array().is_some_and(|outcomes| {
            outcomes.iter().any(|outcome| {
                outcome["kind"] == "create_code_review"
                    && outcome["result"]["reviews"]
                        .as_array()
                        .is_some_and(|reviews| {
                            reviews
                                .iter()
                                .any(|review| review["repository_id"] == branch.repository_id)
                        })
            })
        }) {
            crate::task_pre_pr_review::TaskPrePrReview::description(
                store,
                ctx,
                &crate::task_review_locator::TaskReviewLocator {
                    project: project.ok_or(PlatformError::Forbidden)?,
                    task: &task.id,
                    revision: task.revision,
                    repository: &branch.repository_id,
                    head: &branch.expected_head_sha,
                    review_id: None,
                },
                &target_sha,
            )?
        } else {
            String::new()
        };
        let github = source.connection.provider == GitProvider::Github;
        let endpoint = format!("{root}/{}", if github { "pulls" } else { "merge_requests" });
        let key = format!("{index}:{}", branch.repository_id);
        let marker = format!(
            "<!-- nexora-transition:{}:{key} -->",
            run["id"].as_str().ok_or(PlatformError::Storage)?
        );
        let mut found = None;
        // Reconciliation searches all states, preventing a second PR even if a previous response was lost or the PR closed.
        let mut page = 1u64;
        loop {
            let query = if github {
                format!(
                    "{endpoint}?state=all&head={}&base={}&per_page=100&page={page}",
                    Self::encoded(&format!(
                        "{}:{source_branch}",
                        source
                            .repository
                            .remote_full_name
                            .as_deref()
                            .ok_or(PlatformError::Storage)?
                            .split('/')
                            .next()
                            .ok_or(PlatformError::Storage)?
                    )),
                    Self::encoded(target_branch)
                )
            } else {
                format!(
                    "{endpoint}?state=all&source_branch={}&target_branch={}&per_page=100&page={page}",
                    Self::encoded(source_branch),
                    Self::encoded(target_branch)
                )
            };
            let value = client.get(&query, &token)?;
            let entries = value.as_array().ok_or(PlatformError::Unavailable)?;
            for value in entries {
                if value[if github { "body" } else { "description" }]
                    .as_str()
                    .is_some_and(|body| body.contains(&marker))
                {
                    if found.is_some() {
                        return Err(PlatformError::Conflict(
                            "multiple matching provider requests require administrator reconciliation",
                        ));
                    }
                    found = Some(value.clone());
                }
            }
            if entries.len() < 100 {
                break;
            }
            page = page.checked_add(1).ok_or(PlatformError::Unavailable)?;
        }
        let raw = if let Some(found) = found {
            found
        } else {
            if run["provider_intents"].get(&key).is_some() {
                return Err(PlatformError::Conflict(
                    "previous pull request outcome remains unknown; reconcile it at the provider before retrying",
                ));
            }
            let body = if github {
                json!({"title":title.chars().take(240).collect::<String>(),"head":source_branch,"base":target_branch,"body":format!("Task {}\n\n{marker}{review_description}",run["task_id"].as_str().unwrap_or(""))})
            } else {
                json!({"title":title.chars().take(240).collect::<String>(),"source_branch":source_branch,"target_branch":target_branch,"description":format!("Task {}\n\n{marker}{review_description}",run["task_id"].as_str().unwrap_or("")),"remove_source_branch":false})
            };
            let current_task = TaskTransitionRunStore::authorize(
                store,
                ctx,
                project,
                run["task_id"].as_str().ok_or(PlatformError::Storage)?,
            )?;
            if current_task.revision
                != run["task_revision"]
                    .as_i64()
                    .ok_or(PlatformError::Storage)?
            {
                return Err(PlatformError::Conflict(
                    "task revision changed before provider write",
                ));
            }
            let authority = PullRequestSource::load(
                store,
                ctx,
                project.ok_or(PlatformError::Forbidden)?,
                &branch.repository_id,
                policy,
            )?;
            if !source.matches(&authority) {
                return Err(PlatformError::Conflict(
                    "repository authority changed before provider write",
                ));
            }
            if Self::sha(&client, &source, &token, source_branch)? != branch.expected_head_sha {
                return Err(PlatformError::Conflict(
                    "provider branch head changed before provider write",
                ));
            }
            run["provider_intents"][&key] = json!({"status":"request_started","repository_id":branch.repository_id,"expected_head_sha":branch.expected_head_sha});
            TaskTransitionRunStore::save(store, run)?;
            match client.post(&endpoint, &token, &body) {
                Ok(value) => value,
                Err(
                    error @ (PlatformError::Forbidden
                    | PlatformError::NotFound
                    | PlatformError::Validation(_)
                    | PlatformError::Conflict(_)),
                ) => {
                    // Only an explicit provider rejection proves that no PR was created.
                    run["provider_intents"]
                        .as_object_mut()
                        .ok_or(PlatformError::Storage)?
                        .remove(&key);
                    TaskTransitionRunStore::save(store, run)?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        };
        let summary = crate::pull_request_decoder::PullRequestDecoder::summary(
            source.connection.provider,
            &raw,
        )?;
        if summary["head_sha"] != branch.expected_head_sha
            || summary["source_branch"] != source_branch
            || summary["target_branch"] != target_branch
        {
            return Err(PlatformError::Conflict(
                "pull request head or branches changed; refresh facts and approval",
            ));
        }
        if summary["state"] != "open" {
            return Err(PlatformError::Conflict(
                "reconciled pull request is no longer open; review it before starting another transition",
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
                "repository authority changed during pull request creation",
            ));
        }
        if fact != Self::authority(store, ctx, project, &branch.repository_id)? {
            return Err(PlatformError::Conflict(
                "repository authority or development policy changed after provider verification",
            ));
        }
        store.transition_verified_facts.push(fact);
        run["provider_intents"][&key] = json!({"status":"confirmed","repository_id":branch.repository_id,"number":summary["number"],"head_sha":summary["head_sha"]});
        TaskTransitionRunStore::save(store, run)?;
        Ok(summary)
    }
}
