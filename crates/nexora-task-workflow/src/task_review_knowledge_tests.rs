use super::TaskReviewKnowledge;
use serde_json::json;
#[test]
fn requirement_criteria_keep_exact_version_and_publication_evidence() {
    let input = json!({"kind":"requirement","requirement":{"document_id":"req","version":{"id":"v2","checksum":"digest","content":{"kind":"requirement","body":{"acceptance_criteria":["Atomic operation"]}}},"publication":null}});
    let criteria = TaskReviewKnowledge::criteria(&input);
    assert_eq!(criteria.len(), 1);
    assert_eq!(criteria[0]["id"], "R1C1");
    assert_eq!(criteria[0]["source"]["version_id"], "v2");
    assert!(criteria[0]["source"]["publication"].is_null());
}
#[test]
fn feature_criteria_keep_independent_trace_requirement_versions() {
    let input = json!({"kind":"feature","requirements":[{"requirement":{"document_id":"reqA","version":{"id":"a1","content":{"body":{"acceptance_criteria":["A"]}}}}},{"requirement":{"document_id":"reqB","version":{"id":"b3","content":{"body":{"acceptance_criteria":["B"]}}}}}]});
    let criteria = TaskReviewKnowledge::criteria(&input);
    assert_eq!(criteria.len(), 2);
    assert_eq!(criteria[0]["source"]["version_id"], "a1");
    assert_eq!(criteria[1]["source"]["version_id"], "b3");
    assert_ne!(criteria[0]["id"], criteria[1]["id"]);
}
