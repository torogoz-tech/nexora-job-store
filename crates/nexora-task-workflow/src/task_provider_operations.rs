use crate::pull_request_client::PullRequestClient;
use nexora_platform::{DevelopmentPolicy, GitProvider, PlatformError, TaskWorkBranchReport};
use serde_json::{Value, json};

/// Provider-only operations. Authority and durable write intents are handled by the caller.
pub(crate) struct TaskProviderOperations;
impl TaskProviderOperations {
    fn encoded(value: &str) -> String {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
    pub fn create_branch(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        branch: &str,
        sha: &str,
    ) -> Result<(), PlatformError> {
        if !DevelopmentPolicy::valid_ref(branch) || !TaskWorkBranchReport::valid_sha(sha) {
            return Err(PlatformError::Validation(
                "invalid provider branch creation parameters",
            ));
        }
        let (endpoint, body) = if provider == GitProvider::Github {
            (
                format!("{root}/git/refs"),
                json!({"ref":format!("refs/heads/{branch}"),"sha":sha}),
            )
        } else {
            (
                format!("{root}/repository/branches"),
                json!({"branch":branch,"ref":sha}),
            )
        };
        client.post(&endpoint, token, &body)?;
        Ok(())
    }
    pub fn list(
        client: &PullRequestClient,
        path: &str,
        token: &str,
        field: Option<&str>,
        sha: Option<&str>,
    ) -> Result<Vec<Value>, PlatformError> {
        let mut result = Vec::new();
        let mut page = 1u64;
        let mut expected_total = None;
        let mut previous = std::collections::BTreeSet::new();
        loop {
            let raw = client.get(
                &format!(
                    "{path}{}page={page}",
                    if path.contains('?') { "&" } else { "?" }
                ),
                token,
            )?;
            if sha.is_some_and(|sha| raw["sha"] != sha) {
                return Err(PlatformError::Conflict(
                    "provider returned a different revision",
                ));
            }
            if let Some(total) = raw["total_count"].as_u64() {
                if expected_total.is_some_and(|old| old != total) {
                    return Err(PlatformError::Conflict(
                        "provider results changed during pagination",
                    ));
                }
                expected_total = Some(total);
            }
            let entries = if let Some(field) = field {
                raw[field].as_array()
            } else {
                raw.as_array()
            }
            .ok_or(PlatformError::Unavailable)?;
            for entry in entries {
                if !previous
                    .insert(serde_json::to_string(entry).map_err(|_| PlatformError::Unavailable)?)
                {
                    return Err(PlatformError::Conflict(
                        "provider pagination repeated a record; retry from fresh provider facts",
                    ));
                }
                result.push(entry.clone());
            }
            if entries.len() < 100 {
                break;
            }
            page = page.checked_add(1).ok_or(PlatformError::Unavailable)?;
        }
        if expected_total.is_some_and(|total| total != result.len() as u64) {
            return Err(PlatformError::Conflict("provider response is incomplete"));
        }
        Ok(result)
    }
    pub fn pipeline(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        sha: &str,
        selector: Option<&str>,
    ) -> Result<Value, PlatformError> {
        if provider == GitProvider::Github {
            let runs = Self::list(
                client,
                &format!("{root}/commits/{sha}/check-runs?filter=latest&per_page=100"),
                token,
                Some("check_runs"),
                None,
            )?;
            let selected: Vec<_> = runs
                .iter()
                .filter(|r| selector.is_none_or(|s| r["name"].as_str() == Some(s)))
                .collect();
            if selected.iter().any(|r| {
                r["head_sha"] != sha || r["status"] != "completed" || r["conclusion"] != "success"
            }) {
                return Err(PlatformError::Conflict(
                    "waiting for successful pipeline checks at the pinned revision",
                ));
            }
            let statuses = Self::list(
                client,
                &format!("{root}/commits/{sha}/status?per_page=100"),
                token,
                Some("statuses"),
                Some(sha),
            )?;
            let selected_statuses: Vec<_> = statuses
                .iter()
                .filter(|r| selector.is_none_or(|s| r["context"].as_str() == Some(s)))
                .collect();
            if selected.is_empty() && selected_statuses.is_empty()
                || selected_statuses.iter().any(|s| s["state"] != "success")
            {
                return Err(PlatformError::Conflict(
                    "waiting for successful pipeline checks at the pinned revision",
                ));
            }
            Ok(
                json!({"head_sha":sha,"checks":selected,"statuses":selected_statuses,"status":"success","provenance":"provider_verified"}),
            )
        } else {
            let entries = Self::list(
                client,
                &format!("{root}/pipelines?sha={sha}&order_by=id&sort=desc&per_page=100"),
                token,
                None,
                None,
            )?;
            let pipeline = entries
                .iter()
                .filter(|p| selector.is_none_or(|s| p["name"].as_str() == Some(s)))
                .max_by_key(|p| p["id"].as_u64().unwrap_or(0))
                .ok_or(PlatformError::Conflict(
                    "waiting for a pipeline at the pinned revision",
                ))?;
            if pipeline["sha"] != sha || pipeline["status"] != "success" {
                return Err(PlatformError::Conflict(
                    "waiting for successful pipeline checks at the pinned revision",
                ));
            }
            Ok(
                json!({"head_sha":sha,"pipeline":pipeline,"status":"success","provenance":"provider_verified"}),
            )
        }
    }
    #[cfg(test)]
    pub fn merged(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        branch: &str,
        sha: &str,
    ) -> Result<Value, PlatformError> {
        Self::merged_to(client, provider, root, token, branch, sha, None)
    }
    pub fn merged_to(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        branch: &str,
        sha: &str,
        target: Option<&str>,
    ) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let query = if github {
            format!("{root}/commits/{sha}/pulls?state=closed&per_page=100")
        } else {
            format!(
                "{root}/merge_requests?state=merged&source_branch={}&order_by=updated_at&sort=desc&per_page=100",
                Self::encoded(branch)
            )
        };
        let entries = Self::list(client, &query, token, None, None)?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|p| {
                if github {
                    p["head"]["ref"] == branch
                        && target.is_none_or(|t| p["base"]["ref"] == t)
                        && p["head"]["sha"] == sha
                        && p["merged_at"].is_string()
                } else {
                    p["source_branch"] == branch
                        && target.is_none_or(|t| p["target_branch"] == t)
                        && p["sha"] == sha
                        && p["state"] == "merged"
                }
            })
            .collect();
        if matches.len() != 1 {
            return Err(PlatformError::Conflict(
                "waiting for an unambiguous merged pull request at the pinned revision",
            ));
        }
        let request = matches[0];
        let merge_sha = request["merge_commit_sha"]
            .as_str()
            .filter(|s| TaskWorkBranchReport::valid_sha(s))
            .ok_or(PlatformError::Conflict(
                "provider merge commit revision is not available",
            ))?;
        Ok(
            json!({"head_sha":sha,"merge_sha":merge_sha,"number":request[if github {"number"} else {"iid"}],"target_branch":if github {request["base"]["ref"].clone()} else {request["target_branch"].clone()},"status":"merged","provenance":"provider_verified"}),
        )
    }
    pub fn deployment(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        sha: &str,
        environment: &str,
    ) -> Result<Value, PlatformError> {
        if environment.trim().is_empty() || environment.len() > 256 {
            return Err(PlatformError::Validation(
                "deployment environment is required",
            ));
        }
        let github = provider == GitProvider::Github;
        let query = if github {
            format!(
                "{root}/deployments?environment={}&per_page=100",
                Self::encoded(environment)
            )
        } else {
            format!(
                "{root}/deployments?environment={}&order_by=id&sort=desc&per_page=100",
                Self::encoded(environment)
            )
        };
        let entries = Self::list(client, &query, token, None, None)?;
        let deployment = entries
            .iter()
            .filter(|d| {
                (if github {
                    d["environment"].as_str()
                } else {
                    d["environment"]["name"].as_str()
                }) == Some(environment)
            })
            .max_by_key(|d| d["id"].as_u64().unwrap_or(0))
            .ok_or(PlatformError::Conflict(
                "waiting for deployment to the required environment at the pinned revision",
            ))?;
        if deployment["sha"] != sha {
            return Err(PlatformError::Conflict(
                "waiting for the required revision to be the latest environment deployment",
            ));
        }
        let id = deployment["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or(PlatformError::Unavailable)?;
        let state = if github {
            let statuses = client.get(
                &format!("{root}/deployments/{id}/statuses?per_page=1"),
                token,
            )?;
            statuses
                .as_array()
                .and_then(|a| a.first())
                .and_then(|s| s["state"].as_str())
                .map(str::to_owned)
        } else {
            deployment["status"].as_str().map(str::to_owned)
        };
        if state.as_deref() != Some("success") {
            return Err(PlatformError::Conflict(
                "waiting for successful deployment to the required environment",
            ));
        }
        Ok(
            json!({"head_sha":sha,"environment":environment,"deployment_id":id,"status":"success","provenance":"provider_verified"}),
        )
    }
}

#[cfg(test)]
#[path = "task_provider_operations_tests.rs"]
mod tests;
