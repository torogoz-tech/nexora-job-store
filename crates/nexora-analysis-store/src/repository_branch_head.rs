use nexora_platform::PlatformError;
use serde_json::Value;

pub(crate) struct RepositoryBranchHead;

impl RepositoryBranchHead {
    pub(crate) fn from_github_response(value: &Value) -> Result<String, PlatformError> {
        value["commit"]["sha"]
            .as_str()
            .filter(|sha| crate::git_commit_identity::GitCommitIdentity::valid(sha))
            .map(str::to_owned)
            .ok_or(PlatformError::Unavailable)
    }
}

#[cfg(test)]
#[path = "repository_branch_head_tests.rs"]
mod tests;
