use super::TaskDeliveryEvidence;
use serde_json::json;
#[test]
fn merged_action_and_exact_deployment_prove_delivery() {
    let merged = json!({"repository_id":"repo","target_branch":"stage","provenance":"provider_verified","head_sha":"source","merge_sha":"merged"});
    let deployment = json!({"repository_id":"repo","provenance":"provider_verified","status":"success","source_head_sha":"source","merge":{"target_branch":"stage","merge_sha":"merged"}});
    let mut outcomes = json!([{"kind":"merge_pull_request","result":{"pull_requests":[merged]}},{"kind":"verify_deployment","result":{"deployments":[deployment]}}]);
    assert!(TaskDeliveryEvidence::verify(&outcomes, "stage", false).is_ok());
    outcomes[1]["result"]["deployments"][0]["merge"]["target_branch"] = json!("main");
    assert!(TaskDeliveryEvidence::verify(&outcomes, "stage", false).is_err());
}
#[test]
fn corrective_delivery_requires_same_source_integrated_back_into_develop() {
    let qa = json!({"repository_id":"repo","target_branch":"stage","provenance":"provider_verified","head_sha":"fix","merge_sha":"qa"});
    let deployment = json!({"repository_id":"repo","provenance":"provider_verified","status":"success","source_head_sha":"fix","merge":{"target_branch":"stage","merge_sha":"qa"}});
    let mut outcomes = json!([{"kind":"verify_pull_request_merged","result":{"pull_requests":[qa]}},{"kind":"verify_deployment","result":{"deployments":[deployment]}}]);
    assert!(TaskDeliveryEvidence::verify(&outcomes, "stage", true).is_err());
    outcomes[0]["result"]["pull_requests"].as_array_mut().unwrap().push(json!({"repository_id":"repo","target_branch":"develop","provenance":"provider_verified","head_sha":"fix","merge_sha":"dev"}));
    assert!(TaskDeliveryEvidence::verify(&outcomes, "stage", true).is_ok());
}
