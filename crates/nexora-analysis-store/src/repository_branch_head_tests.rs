use crate::repository_branch_head::RepositoryBranchHead;
use nexora_platform::PlatformError;
use serde_json::json;

#[test]
fn accepts_a_valid_github_commit_identity() {
    let sha = "a".repeat(40);

    assert_eq!(
        RepositoryBranchHead::from_github_response(&json!({"commit":{"sha":sha}})).unwrap(),
        sha
    );
}

#[test]
fn rejects_missing_or_malformed_github_commit_identity() {
    assert!(matches!(
        RepositoryBranchHead::from_github_response(&json!({"commit":{}})),
        Err(PlatformError::Unavailable)
    ));
    assert!(matches!(
        RepositoryBranchHead::from_github_response(&json!({"commit":{"sha":"not-a-sha"}})),
        Err(PlatformError::Unavailable)
    ));
}
