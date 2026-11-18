use crate::{GitNetworkPolicy, TaskWorkflowStore};
use nexora_platform::*;
use serde_json::Value;
/// Restores live provider authority when a durable run resumes after its AdvanceState checkpoint.
pub(crate) struct TaskHandoffProviderEvidence;
impl TaskHandoffProviderEvidence {
    fn branch<'a>(
        input: &'a TaskTransitionRequest,
        fact: &Value,
        head: &str,
    ) -> Result<&'a TaskTransitionBranch, PlatformError> {
        let repository = fact["repository_id"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let source = fact["source_branch"]
            .as_str()
            .or(fact["branch_name"].as_str());
        let matching: Vec<_> = input
            .branches
            .iter()
            .filter(|branch| {
                branch.repository_id == repository
                    && branch.expected_head_sha == head
                    && source.is_none_or(|source| branch.branch_name == source)
            })
            .collect();
        if matching.len() != 1 {
            return Err(PlatformError::Conflict(
                "handoff replay requires an unambiguous pinned source branch for every provider receipt",
            ));
        }
        Ok(matching[0])
    }
    pub fn refresh(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        input: &TaskTransitionRequest,
        policy: &GitNetworkPolicy,
        outcomes: &Value,
    ) -> Result<(), PlatformError> {
        let steps = outcomes.as_array().ok_or(PlatformError::Storage)?;
        for step in steps {
            match step["kind"].as_str() {
                Some("verify_pull_request_merged" | "merge_pull_request") => {
                    for receipt in step["result"]["pull_requests"]
                        .as_array()
                        .ok_or(PlatformError::Storage)?
                    {
                        let head = receipt["head_sha"].as_str().ok_or(PlatformError::Storage)?;
                        let branch = Self::branch(input, receipt, head)?;
                        let target = receipt["target_branch"]
                            .as_str()
                            .ok_or(PlatformError::Storage)?;
                        let current =
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
                                    target_branch: Some(target),
                                },
                            )?;
                        if current["merge_sha"] != receipt["merge_sha"]
                            || current["head_sha"] != receipt["head_sha"]
                            || current["target_branch"] != receipt["target_branch"]
                        {
                            return Err(PlatformError::Conflict(
                                "provider merge evidence changed before handoff",
                            ));
                        }
                    }
                }
                Some("verify_deployment") => {
                    for receipt in step["result"]["deployments"]
                        .as_array()
                        .ok_or(PlatformError::Storage)?
                    {
                        let head = receipt["source_head_sha"]
                            .as_str()
                            .ok_or(PlatformError::Storage)?;
                        let branch = Self::branch(input, receipt, head)?;
                        let target = receipt["merge"]["target_branch"]
                            .as_str()
                            .ok_or(PlatformError::Storage)?;
                        let environment = receipt["environment"]
                            .as_str()
                            .ok_or(PlatformError::Storage)?;
                        let current =
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
                                    target_branch: Some(target),
                                },
                            )?;
                        if current["head_sha"] != receipt["head_sha"]
                            || current["source_head_sha"] != receipt["source_head_sha"]
                            || current["merge"]["merge_sha"] != receipt["merge"]["merge_sha"]
                            || current["environment"] != receipt["environment"]
                        {
                            return Err(PlatformError::Conflict(
                                "provider deployment evidence changed before handoff",
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "task_handoff_provider_evidence_tests.rs"]
mod tests;
