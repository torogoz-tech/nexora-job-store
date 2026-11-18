use crate::task_transition_engine::TaskTransitionEngine;
use nexora_platform::TaskTransitionBranch;
use serde_json::json;
#[test]
fn later_merge_reconciles_creation_with_exact_revision_repository_and_target() {
    let branch = TaskTransitionBranch {
        repository_id: "repo".into(),
        branch_name: "work/correction".into(),
        expected_head_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    };
    let run = json!({"outcomes":[{"kind":"merge_pull_request","result":{"pull_requests":[
        {"repository_id":"repo","head_sha":branch.expected_head_sha,"target_branch":"qa","status":"merged"},
        {"repository_id":"repo","head_sha":branch.expected_head_sha,"target_branch":"develop","status":"merged"}
    ]}}]});
    assert_eq!(
        TaskTransitionEngine::merged_branch(&run, &branch, Some("qa")).unwrap()["target_branch"],
        "qa"
    );
    assert!(TaskTransitionEngine::merged_branch(&run, &branch, Some("production")).is_none());
    let mut moved = branch;
    moved.expected_head_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    assert!(TaskTransitionEngine::merged_branch(&run, &moved, None).is_none());
}

#[test]
fn postmerge_pipeline_uses_the_exact_preceding_merge_boundary() {
    let branch = TaskTransitionBranch {
        repository_id: "repo".into(),
        branch_name: "work/correction".into(),
        expected_head_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    };
    let run = json!({"outcomes":[
        {"step_index":0,"kind":"merge_pull_request","result":{"pull_requests":[{"repository_id":"repo","head_sha":branch.expected_head_sha,"target_branch":"qa","status":"merged"}]}},
        {"step_index":2,"kind":"merge_pull_request","result":{"pull_requests":[{"repository_id":"repo","head_sha":branch.expected_head_sha,"target_branch":"develop","status":"merged"}]}}
    ]});
    assert!(TaskTransitionEngine::merged_before(&run, &branch, 0).is_none());
    assert_eq!(
        TaskTransitionEngine::merged_before(&run, &branch, 1).unwrap()["target_branch"],
        "qa"
    );
    assert_eq!(
        TaskTransitionEngine::merged_before(&run, &branch, 3).unwrap()["target_branch"],
        "develop"
    );
}
