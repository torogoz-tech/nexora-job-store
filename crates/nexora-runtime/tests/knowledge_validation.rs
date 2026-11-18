use nexora_runtime::KnowledgeCommand;
use serde_json::{Value, json};

fn draft() -> Value {
    json!({"kind":"relation","statement":"handler calls service","source":"node:a","target":"node:b","relation":"CALLS","evidence":[{"path":"a.tsx","start_line":1,"line_count":2}]})
}
#[test]
fn contributions_require_bounded_evidence_and_relation_endpoints() {
    let valid = json!({"operation":"contribute","request_id":"request-1","draft":draft()});
    let command: KnowledgeCommand = serde_json::from_value(valid.clone()).unwrap();
    assert!(command.validate().is_ok());
    for field in ["source", "target", "relation"] {
        let mut invalid = valid.clone();
        invalid["draft"].as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<KnowledgeCommand>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    for evidence in [
        json!([]),
        json!([{"path":"../a","start_line":1,"line_count":1}]),
        json!([{"path":"a","start_line":0,"line_count":1}]),
        json!([{"path":"a","start_line":1,"line_count":201}]),
    ] {
        let mut invalid = valid.clone();
        invalid["draft"]["evidence"] = evidence;
        assert!(
            serde_json::from_value::<KnowledgeCommand>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut invalid = valid;
    invalid["draft"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<KnowledgeCommand>(invalid).is_err());
}
#[test]
fn reviews_and_lists_reject_invalid_states_and_concurrency_versions() {
    for input in [
        json!({"operation":"review","id":"k","request_id":"r","expected_revision":0,"action":"accept","reason":"checked"}),
        json!({"operation":"review","id":"k","request_id":"r","expected_revision":1,"action":"invented","reason":"checked"}),
        json!({"operation":"list","status":"invented","offset":0,"limit":10}),
        json!({"operation":"list","offset":0,"limit":101}),
    ] {
        assert!(
            serde_json::from_value::<KnowledgeCommand>(input)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
