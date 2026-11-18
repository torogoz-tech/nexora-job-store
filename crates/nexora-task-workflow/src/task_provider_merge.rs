use crate::{
    pull_request_client::PullRequestClient, task_provider_operations::TaskProviderOperations,
};
use nexora_platform::{GitProvider, PlatformError, TaskWorkBranchReport};
use serde_json::{Value, json};
pub(crate) struct TaskProviderMerge;
impl TaskProviderMerge {
    fn encoded(value: &str) -> String {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
    pub fn find(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        branch: &str,
        head: &str,
        target: &str,
    ) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let query = if github {
            format!("{root}/commits/{head}/pulls?state=all&per_page=100")
        } else {
            format!(
                "{root}/merge_requests?state=all&source_branch={}&target_branch={}&per_page=100",
                Self::encoded(branch),
                Self::encoded(target)
            )
        };
        let entries = TaskProviderOperations::list(client, &query, token, None, None)?;
        let matching: Vec<_> = entries
            .iter()
            .filter(|p| {
                if github {
                    p["head"]["ref"] == branch
                        && p["head"]["sha"] == head
                        && p["base"]["ref"] == target
                        && (p["state"] == "open" || p["merged_at"].is_string())
                } else {
                    p["source_branch"] == branch
                        && p["sha"] == head
                        && p["target_branch"] == target
                        && (p["state"] == "opened" || p["state"] == "merged")
                }
            })
            .collect();
        if matching.len() != 1 {
            return Err(PlatformError::Conflict(
                "waiting for one matching open or merged pull request at the pinned revision and target",
            ));
        }
        let number = matching[0][if github { "number" } else { "iid" }]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or(PlatformError::Unavailable)?;
        let detail = client.get(
            &format!(
                "{root}/{}/{number}",
                if github { "pulls" } else { "merge_requests" }
            ),
            token,
        )?;
        let summary = crate::pull_request_decoder::PullRequestDecoder::summary(provider, &detail)?;
        if summary["head_sha"] != head
            || summary["source_branch"] != branch
            || summary["target_branch"] != target
        {
            return Err(PlatformError::Conflict(
                "pull request revision or target changed before merge",
            ));
        }
        Ok(detail)
    }
    pub fn confirmed(
        provider: GitProvider,
        raw: &Value,
        head: &str,
        target: &str,
    ) -> Result<Option<Value>, PlatformError> {
        let github = provider == GitProvider::Github;
        let merged = if github {
            raw["merged"] == true || raw["merged_at"].is_string()
        } else {
            raw["state"] == "merged"
        };
        if !merged {
            return Ok(None);
        }
        let sha = raw["merge_commit_sha"]
            .as_str()
            .filter(|sha| TaskWorkBranchReport::valid_sha(sha))
            .ok_or(PlatformError::Conflict(
                "waiting for the provider-confirmed merge commit revision",
            ))?;
        Ok(Some(
            json!({"number":raw[if github{"number"}else{"iid"}],"head_sha":head,"merge_sha":sha,"target_branch":target,"status":"merged","provenance":"provider_verified"}),
        ))
    }
    pub fn ready(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        raw: &Value,
        head: &str,
        target: &str,
    ) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let number = raw[if github { "number" } else { "iid" }]
            .as_u64()
            .ok_or(PlatformError::Unavailable)?;
        if github {
            if raw["draft"] != false
                || raw["mergeable"] != true
                || raw["mergeable_state"] != "clean"
            {
                return Err(PlatformError::Conflict(
                    "waiting for a non-draft conflict-free pull request with all required provider gates satisfied",
                ));
            }
            let rules = TaskProviderOperations::list(
                client,
                &format!(
                    "{root}/rules/branches/{}?per_page=100",
                    Self::encoded(target)
                ),
                token,
                None,
                None,
            )?;
            if rules.iter().any(|r| r["type"] == "merge_queue") {
                return Err(PlatformError::Conflict(
                    "provider merge queue is required; direct merge is forbidden",
                ));
            }
            let protection = match client.get(
                &format!("{root}/branches/{}/protection", Self::encoded(target)),
                token,
            ) {
                Ok(p) => p,
                Err(PlatformError::NotFound) => Value::Null,
                Err(e) => return Err(e),
            };
            let rule_count = rules
                .iter()
                .filter(|r| r["type"] == "pull_request")
                .map(|r| {
                    r["parameters"]["required_approving_review_count"]
                        .as_u64()
                        .ok_or(PlatformError::Unavailable)
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .max()
                .unwrap_or(0);
            let required = rule_count.max(
                protection["required_pull_request_reviews"]["required_approving_review_count"]
                    .as_u64()
                    .unwrap_or(0),
            );
            let reviews = TaskProviderOperations::list(
                client,
                &format!("{root}/pulls/{number}/reviews?per_page=100"),
                token,
                None,
                None,
            )?;
            let mut latest = std::collections::BTreeMap::<u64, &Value>::new();
            for review in &reviews {
                let state = review["state"].as_str().ok_or(PlatformError::Unavailable)?;
                if !["APPROVED", "CHANGES_REQUESTED", "DISMISSED"].contains(&state) {
                    continue;
                }
                let user = review["user"]["id"]
                    .as_u64()
                    .ok_or(PlatformError::Unavailable)?;
                let id = review["id"].as_u64().ok_or(PlatformError::Unavailable)?;
                if latest
                    .get(&user)
                    .is_none_or(|old| old["id"].as_u64().unwrap_or(0) < id)
                {
                    latest.insert(user, review);
                }
            }
            if latest.values().any(|r| r["state"] == "CHANGES_REQUESTED")
                || latest
                    .values()
                    .filter(|r| r["state"] == "APPROVED" && r["commit_id"] == head)
                    .count()
                    < (required as usize)
            {
                return Err(PlatformError::Conflict(
                    "waiting for provider pull request approvals at the exact pinned revision",
                ));
            }
            TaskProviderOperations::pipeline(client, provider, root, token, head, None)?;
            Ok(
                json!({"rules":rules,"protection":protection,"required_approvals":required,"head_sha":head,"status":"ready"}),
            )
        } else {
            if raw["draft"] != false
                || raw["has_conflicts"] != false
                || raw["detailed_merge_status"] != "mergeable"
            {
                return Err(PlatformError::Conflict(
                    "waiting for a non-draft conflict-free merge request with all required provider gates satisfied",
                ));
            }
            let approvals =
                client.get(&format!("{root}/merge_requests/{number}/approvals"), token)?;
            if approvals["approvals_left"].as_u64() != Some(0) {
                return Err(PlatformError::Conflict(
                    "waiting for provider merge request approvals",
                ));
            }
            TaskProviderOperations::pipeline(client, provider, root, token, head, None)?;
            Ok(json!({"approvals":approvals,"head_sha":head,"status":"ready"}))
        }
    }
    pub fn write(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        number: u64,
        head: &str,
    ) -> Result<Value, PlatformError> {
        if !TaskWorkBranchReport::valid_sha(head) || number == 0 {
            return Err(PlatformError::Validation(
                "merge requires an exact head SHA and request number",
            ));
        }
        let github = provider == GitProvider::Github;
        client.put(
            &format!(
                "{root}/{}/{number}/merge",
                if github { "pulls" } else { "merge_requests" }
            ),
            token,
            &if github {
                json!({"sha":head,"merge_method":"merge"})
            } else {
                json!({"sha":head,"auto_merge":false,"should_remove_source_branch":false})
            },
        )
    }
}

#[cfg(test)]
#[path = "task_provider_merge_tests.rs"]
mod tests;
