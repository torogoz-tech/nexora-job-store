use crate::{TaskWorkflowStore, task_release_store::TaskReleaseStore};
use nexora_platform::*;
use serde_json::{Value, json};
pub(crate) struct TaskReleasePublication;
impl TaskReleasePublication {
    pub fn selection(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        input: &TaskTransitionRequest,
        run: &Value,
    ) -> Result<(), PlatformError> {
        let project = project.ok_or(PlatformError::Validation(
            "release publication requires a project",
        ))?;
        TaskReleaseStore::authorize_publication(store, ctx, project)?;
        let id = input
            .release_id
            .as_deref()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or(PlatformError::Validation(
                "select a release before running its final pipeline",
            ))?;
        let revision = input
            .release_revision
            .filter(|revision| *revision > 0)
            .ok_or(PlatformError::Validation("release revision is required"))?;
        let release = TaskReleaseStore::get(store, ctx, project, id)?;
        if release["status"] == "published"
            && release["publication_evidence"]["transition_run_id"] == run["id"]
            && release["revision"].as_i64() == Some(revision + 1)
        {
            return Ok(());
        }
        if release["status"] != "prepared" || release["revision"].as_i64() != Some(revision) {
            return Err(PlatformError::Conflict(
                "selected release changed before final pipeline execution",
            ));
        }
        TaskReleaseStore::validate_pins(store, project, id)?;
        let repositories = release["repositories"]
            .as_array()
            .ok_or(PlatformError::Storage)?;
        let branch = release["branch_name"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        if repositories.iter().any(|repository| {
            repository["composition_status"] != "composed"
                || repository["composition_evidence"]["provenance"] != "server_composed"
        }) {
            return Err(PlatformError::Conflict(
                "compose the selected task commits before running the release pipeline",
            ));
        }
        if repositories.is_empty()
            || input.branches.len() != repositories.len()
            || repositories.iter().any(|repository| {
                input
                    .branches
                    .iter()
                    .filter(|candidate| {
                        candidate.repository_id == repository["repository_id"]
                            && candidate.branch_name == branch
                            && candidate.expected_head_sha == repository["head_sha"]
                    })
                    .count()
                    != 1
            })
        {
            return Err(PlatformError::Validation(
                "release pipeline branch facts must exactly match every selected release repository and commit",
            ));
        }
        Ok(())
    }
    pub fn execute(
        execution: &mut crate::task_transition_execution_context::TaskTransitionExecutionContext<
            '_,
        >,
    ) -> Result<Value, PlatformError> {
        let store = &mut *execution.store;
        let ctx = execution.ctx;
        let project = execution.project;
        let input = execution.input;
        let policy = execution.policy;
        let run = &*execution.run;
        let release_id = input
            .release_id
            .as_deref()
            .ok_or(PlatformError::Validation(
                "select a release before running its final pipeline",
            ))?;
        let revision = input
            .release_revision
            .ok_or(PlatformError::Validation("release revision is required"))?;
        let project = project.ok_or(PlatformError::Validation(
            "release publication requires a project",
        ))?;
        let outcomes = run["outcomes"].as_array().ok_or(PlatformError::Storage)?;
        if input.approval_id.is_none()
            || run["advanced"] != true
            || !outcomes.iter().any(|o| {
                o["kind"] == "verify_pull_request_merged" || o["kind"] == "merge_pull_request"
            })
            || !outcomes.iter().any(|o| o["kind"] == "verify_deployment")
        {
            return Err(PlatformError::Conflict(
                "release publication requires approved merged pull request and production deployment steps",
            ));
        }
        let release = TaskReleaseStore::get(store, ctx, project, release_id)?;
        if release["status"] == "published" {
            if release["revision"].as_i64() == Some(revision + 1)
                && release["publication_evidence"]["transition_run_id"] == run["id"]
            {
                return Ok(release);
            }
            return Err(PlatformError::Conflict(
                "release is already published by another transition",
            ));
        }
        let branch_name = release["branch_name"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let repositories = release["repositories"]
            .as_array()
            .ok_or(PlatformError::Storage)?;
        let mut verified = Vec::new();
        let mut confirmations = Vec::new();
        for repository in repositories {
            let repository_id = repository["repository_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let pinned = repository["head_sha"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let candidates: Vec<_> = input
                .branches
                .iter()
                .filter(|branch| {
                    branch.repository_id == repository_id
                        && branch.branch_name == branch_name
                        && branch.expected_head_sha == pinned
                })
                .collect();
            if candidates.len() != 1 {
                return Err(PlatformError::Conflict(
                    "release requires exact branch and head facts for every selected repository",
                ));
            }
            let branch = candidates[0];
            let proof = &repository["composition_evidence"];
            if repository["composition_status"] != "composed"
                || proof["provenance"] != "server_composed"
                || proof["head_sha"] != pinned
                || proof["branch_name"] != branch_name
            {
                return Err(PlatformError::Conflict(
                    "release requires server-composed exact selected commit evidence",
                ));
            }
            let selected = release["commits"]
                .as_array()
                .ok_or(PlatformError::Storage)?;
            let expected: std::collections::BTreeSet<&str> = selected
                .iter()
                .filter(|item| item["repository_id"] == repository_id)
                .filter_map(|item| item["commit_sha"].as_str())
                .collect();
            let actual: std::collections::BTreeSet<&str> = proof["commits"]
                .as_array()
                .ok_or(PlatformError::Storage)?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            if expected.is_empty() || expected != actual {
                return Err(PlatformError::Conflict(
                    "release composition must contain exactly the selected task commits and no extra commits",
                ));
            }
            let merged = crate::task_transition_provider::TaskTransitionProvider::observe_to(
                crate::provider_request_context::ProviderRequestContext {
                    store,
                    ctx,
                    project: Some(project),
                    policy,
                },
                crate::provider_observation_request::ProviderObservationRequest {
                    branch,
                    observation: "pull_request_merged",
                    selector: None,
                    target_branch: Some("main"),
                },
            )?;
            let deployed = crate::task_transition_provider::TaskTransitionProvider::observe_to(
                crate::provider_request_context::ProviderRequestContext {
                    store,
                    ctx,
                    project: Some(project),
                    policy,
                },
                crate::provider_observation_request::ProviderObservationRequest {
                    branch,
                    observation: "deployment",
                    selector: Some("production"),
                    target_branch: Some("main"),
                },
            )?;
            if merged["provenance"] != "provider_verified"
                || merged["status"] != "merged"
                || merged["target_branch"] != "main"
                || merged["head_sha"] != pinned
                || deployed["provenance"] != "provider_verified"
                || deployed["status"] != "success"
                || deployed["environment"] != "production"
                || deployed["source_head_sha"] != pinned
                || deployed["head_sha"] != merged["merge_sha"]
                || deployed["merge"]["merge_sha"] != merged["merge_sha"]
                || deployed["merge"]["target_branch"] != "main"
            {
                return Err(PlatformError::Conflict(
                    "production deployment must verify the exact selected release merge SHA on main",
                ));
            }
            verified.push((repository_id.to_owned(), pinned.to_owned()));
            confirmations
                .push(json!({"repository_id":repository_id,"merged":merged,"deployed":deployed}));
        }
        TaskReleaseStore::publish(
            store,
            ctx,
            project,
            release_id,
            revision,
            &verified,
            &json!({"transition_run_id":run["id"],"approval_id":input.approval_id,"provenance":"provider_verified","repositories":confirmations}),
        )
    }
}
