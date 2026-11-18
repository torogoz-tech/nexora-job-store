use super::TaskHandoffProviderEvidence;
use nexora_platform::{TaskTransitionBranch, TaskTransitionRequest};
use serde_json::json;
#[test]
fn replay_uses_saved_source_identity_and_never_picks_an_arbitrary_branch_at_same_sha() {
    let mut request = TaskTransitionRequest::default();
    for name in ["work/task", "qa"] {
        request.branches.push(TaskTransitionBranch {
            repository_id: "repo".into(),
            branch_name: name.into(),
            expected_head_sha: "a".repeat(40),
        });
    }
    let fact = json!({"repository_id":"repo","source_branch":"work/task"});
    assert_eq!(
        TaskHandoffProviderEvidence::branch(&request, &fact, &"a".repeat(40))
            .unwrap()
            .branch_name,
        "work/task"
    );
    assert!(
        TaskHandoffProviderEvidence::branch(
            &request,
            &json!({"repository_id":"repo"}),
            &"a".repeat(40)
        )
        .is_err()
    );
    assert!(TaskHandoffProviderEvidence::branch(&request, &fact, &"b".repeat(40)).is_err());
}
