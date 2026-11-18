use nexora_platform::{GitProvider, PlatformError};
use serde_json::{Value, json};
pub struct PullRequestDecoder;
impl PullRequestDecoder {
    fn text(value: &Value, key: &str, max: usize) -> String {
        value[key]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(max)
            .collect()
    }
    pub fn summary(provider: GitProvider, v: &Value) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let number = v[if github { "number" } else { "iid" }]
            .as_u64()
            .filter(|v| *v > 0 && *v <= i64::MAX as u64)
            .ok_or(PlatformError::Unavailable)?;
        let title = Self::text(v, "title", 512);
        if title.is_empty() {
            return Err(PlatformError::Unavailable);
        }
        let merged = if github {
            v["merged"].as_bool() == Some(true) || v["merged_at"].is_string()
        } else {
            v["state"] == "merged"
        };
        let state = if merged {
            "merged"
        } else if ["closed", "locked"].contains(&v["state"].as_str().unwrap_or("")) {
            "closed"
        } else {
            "open"
        };
        Ok(
            json!({"number":number,"title":title,"description":Self::text(v,if github{"body"}else{"description"},65536),"state":state,"draft":v["draft"].as_bool().unwrap_or(false),"web_url":Self::text(v,if github{"html_url"}else{"web_url"},2048),"author":if github{Self::text(&v["user"],"login",256)}else{Self::text(&v["author"],"username",256)},"source_branch":if github{Self::text(&v["head"],"ref",512)}else{Self::text(v,"source_branch",512)},"target_branch":if github{Self::text(&v["base"],"ref",512)}else{Self::text(v,"target_branch",512)},"head_sha":if github{Self::text(&v["head"],"sha",64)}else{Self::text(v,"sha",64)},"base_sha":if github{Self::text(&v["base"],"sha",64)}else{Self::text(&v["diff_refs"],"base_sha",64)},"changed_files":v["changed_files"].as_u64(),"created_at":Self::text(v,"created_at",64),"updated_at":Self::text(v,"updated_at",64),"mergeable":v["mergeable"].as_bool(),"additions":v["additions"].as_u64(),"deletions":v["deletions"].as_u64()}),
        )
    }
}
