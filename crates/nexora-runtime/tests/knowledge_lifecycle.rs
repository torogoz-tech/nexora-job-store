mod common;

use nexora_core::TenantId;
use nexora_indexer::Indexer;
use nexora_language::LanguageDefinition;
use nexora_runtime::{DatabaseConnections, DatabaseRuntime, KnowledgeCommand};
use serde_json::{Value, json};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn command(value: Value) -> KnowledgeCommand {
    serde_json::from_value(value).unwrap()
}
#[test]
#[ignore = "requires real local databases; public generated sources only"]
fn contributions_are_durable_scoped_revisioned_and_invalidated_by_new_snapshots() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-knowledge-{nonce}"));
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.js"), "function hello() {}\n// evidence 🐦\n").unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("public-knowledge-{nonce}"));
    let indexed = runtime.index(&indexer, &dir, tenant.clone()).unwrap();
    let repo = indexed.snapshot.repository_id.clone();
    let snapshot = indexed.snapshot.id.clone();
    let target = indexed
        .snapshot
        .graph
        .nodes
        .iter()
        .find(|n| n.name == "hello")
        .unwrap()
        .id
        .0
        .clone();
    let draft = json!({"kind":"relation","statement":"This file exposes the greeting entry point.","source":"file:a.js","target":target,"relation":"EXPOSES","evidence":[{"path":"a.js","start_line":1,"line_count":2}]});
    let request = command(json!({"operation":"contribute","request_id":"create-1","draft":draft}));
    let first = runtime
        .knowledge(&tenant, &repo, Some(&snapshot), "public-agent", &request)
        .unwrap();
    assert_eq!(first["contribution"]["state"], "proposed");
    assert_eq!(first["contribution"]["origin"], "contributed");
    assert_eq!(first["contribution"]["actor"], "public-agent");
    assert_eq!(first["contribution"]["effective"], false);
    let id = first["contribution"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        runtime
            .knowledge(&tenant, &repo, Some(&snapshot), "public-agent", &request)
            .unwrap(),
        first
    );
    let mut altered = serde_json::to_value(&request).unwrap();
    altered["draft"]["statement"] = json!("changed");
    assert!(
        runtime
            .knowledge(
                &tenant,
                &repo,
                Some(&snapshot),
                "public-agent",
                &command(altered)
            )
            .is_err()
    );
    for change in [
        json!({"source":"missing"}),
        json!({"evidence":[{"path":"absent.js","start_line":1,"line_count":1}]}),
        json!({"evidence":[{"path":"a.js","start_line":2,"line_count":2}]}),
    ] {
        let mut bad = draft.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(runtime.knowledge(&tenant,&repo,Some(&snapshot),"public-agent",&command(json!({"operation":"contribute","request_id":format!("invalid-{change}"),"draft":bad}))).is_err());
    }
    let accept = command(
        json!({"operation":"review","id":id,"request_id":"accept-1","expected_revision":1,"action":"accept","reason":"Verified against source"}),
    );
    let accepted = runtime
        .knowledge(&tenant, &repo, Some(&snapshot), "public-reviewer", &accept)
        .unwrap();
    assert_eq!(accepted["contribution"]["revision"], 2);
    assert_eq!(accepted["contribution"]["effective"], true);
    assert!(runtime.knowledge(&tenant,&repo,Some(&snapshot),"other-reviewer",&command(json!({"operation":"review","id":id,"request_id":"stale","expected_revision":1,"action":"reject","reason":"stale write"}))).is_err());
    drop(runtime);
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let get = command(json!({"operation":"get","id":id,"offset":0,"limit":1}));
    let detail = runtime.knowledge(&tenant, &repo, None, "", &get).unwrap();
    assert_eq!(detail["contribution"]["state"], "accepted");
    assert_eq!(detail["history_total"], 2);
    assert_eq!(detail["next_offset"], 1);
    assert_eq!(detail["history"][0]["revision"], 1);
    assert!(
        runtime
            .knowledge(&TenantId("other".into()), &repo, Some(&snapshot), "", &get)
            .is_err()
    );
    let list = command(
        json!({"operation":"list","status":"accepted","node":target,"offset":0,"limit":10}),
    );
    assert_eq!(
        runtime.knowledge(&tenant, &repo, None, "", &list).unwrap()["total"],
        1
    );
    fs::write(dir.join("a.js"), "function hello() { return 2; }\n").unwrap();
    let updated = runtime.index(&indexer, &dir, tenant.clone()).unwrap();
    assert_ne!(snapshot, updated.snapshot.id);
    let stale = runtime.knowledge(&tenant, &repo, None, "", &get).unwrap();
    assert_eq!(stale["contribution"]["validity"], "needs_review");
    assert_eq!(stale["contribution"]["effective"], false);
    assert_eq!(
        runtime
            .knowledge(&tenant, &repo, Some(&snapshot), "", &get)
            .unwrap()["contribution"]["effective"],
        true
    );
    assert_eq!(
        runtime
            .knowledge(
                &tenant,
                &repo,
                None,
                "",
                &command(json!({"operation":"list","status":"needs_review","offset":0,"limit":10}))
            )
            .unwrap()["total"],
        1
    );
    let mut revised = draft.clone();
    revised["evidence"][0]["line_count"] = json!(1);
    revised["target"] = json!(
        updated
            .snapshot
            .graph
            .nodes
            .iter()
            .find(|n| n.name == "hello")
            .unwrap()
            .id
            .0
    );
    let correction = command(
        json!({"operation":"revise","id":id,"request_id":"revise-1","expected_revision":2,"draft":revised}),
    );
    let corrected = runtime
        .knowledge(&tenant, &repo, None, "public-agent", &correction)
        .unwrap();
    assert_eq!(corrected["contribution"]["state"], "proposed");
    assert_eq!(corrected["contribution"]["validity"], "current");
    let withdraw = command(
        json!({"operation":"review","id":id,"request_id":"withdraw-1","expected_revision":3,"action":"withdraw","reason":"Superseded design"}),
    );
    let withdrawn = runtime
        .knowledge(&tenant, &repo, None, "public-agent", &withdraw)
        .unwrap();
    assert_eq!(withdrawn["contribution"]["state"], "withdrawn");
    assert_eq!(withdrawn["contribution"]["effective"], false);
    assert_eq!(
        runtime
            .knowledge(
                &tenant,
                &repo,
                None,
                "",
                &command(json!({"operation":"get","id":id,"offset":0,"limit":100}))
            )
            .unwrap()["history_total"],
        4
    );
    let pending = runtime
        .knowledge_pending(&tenant, &repo, None, 0, 10)
        .unwrap();
    assert!(pending["items"].is_array());
    assert_eq!(
        runtime
            .snapshot(&tenant, &repo, Some(&snapshot))
            .unwrap()
            .unwrap()
            .graph
            .edges
            .len(),
        indexed.snapshot.graph.edges.len()
    );
    let replay = runtime
        .knowledge(&tenant, &repo, Some(&snapshot), "public-reviewer", &accept)
        .unwrap();
    assert_eq!(replay["contribution"]["effective"], false);
    assert_eq!(replay["contribution"]["is_latest"], false);
    let history = runtime
        .knowledge(
            &tenant,
            &repo,
            Some(&snapshot),
            "",
            &command(json!({"operation":"get","id":id,"offset":0,"limit":100})),
        )
        .unwrap();
    assert_eq!(history["history"][1]["state"], "accepted");
    assert_eq!(history["history"][1]["effective"], false);
    assert_eq!(history["history"][1]["is_latest"], false);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires real local databases; public fixture and concurrent clients"]
fn pending_edges_match_analysis_and_concurrent_reviews_preserve_one_winner() {
    use nexora_graph::ResolutionState;
    use std::sync::{Arc, Barrier};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("public-knowledge-race-{nonce}"));
    let indexed = runtime
        .index(
            &indexer,
            common::domain_fixture_path("ambiguous").as_path(),
            tenant.clone(),
        )
        .unwrap();
    let repo = indexed.snapshot.repository_id.clone();
    let snapshot = indexed.snapshot.id.clone();
    let expected = indexed
        .snapshot
        .graph
        .edges
        .iter()
        .filter(|e| e.resolution != ResolutionState::Resolved)
        .map(|e| serde_json::to_value(e).unwrap())
        .collect::<Vec<_>>();
    assert!(!expected.is_empty());
    let pending = runtime
        .knowledge_pending(&tenant, &repo, Some(&snapshot), 0, 100)
        .unwrap();
    assert_eq!(pending["items"], json!(expected));
    let page = runtime
        .knowledge_pending(&tenant, &repo, Some(&snapshot), 0, 1)
        .unwrap();
    assert_eq!(page["items"], json!([expected[0]]));
    assert_eq!(page["total"], expected.len());
    let path = indexed.snapshot.sources.keys().next().unwrap();
    let create = command(
        json!({"operation":"contribute","request_id":"race-create","draft":{"kind":"decision","statement":"The ambiguous boundary needs explicit review.","evidence":[{"path":path,"start_line":1,"line_count":1}]}}),
    );
    let created = runtime
        .knowledge(&tenant, &repo, Some(&snapshot), "agent", &create)
        .unwrap();
    let id = created["contribution"]["id"].as_str().unwrap().to_owned();
    let barrier = Arc::new(Barrier::new(2));
    let workers=(0..2).map(|i|{
        let config=config.clone();let tenant=tenant.clone();let repo=repo.clone();let snapshot=snapshot.clone();let id=id.clone();let barrier=barrier.clone();
        std::thread::spawn(move||{
            let mut runtime=DatabaseRuntime::connect(&config).unwrap();
            let review=command(json!({"operation":"review","id":id,"request_id":format!("race-{i}"),"expected_revision":1,"action":"accept","reason":"Concurrent source review"}));
            barrier.wait();
            runtime.knowledge(&tenant,&repo,Some(&snapshot),"reviewer",&review).is_ok()
        })
    }).collect::<Vec<_>>();
    assert_eq!(
        workers
            .into_iter()
            .filter_map(|w| w.join().unwrap().then_some(()))
            .count(),
        1
    );
    let detail = runtime
        .knowledge(
            &tenant,
            &repo,
            None,
            "",
            &command(json!({"operation":"get","id":id,"offset":0,"limit":100})),
        )
        .unwrap();
    assert_eq!(detail["history_total"], 2);
    assert_eq!(detail["history"].as_array().unwrap().len(), 2);
    // Another existing repository in the same tenant cannot see this contribution.
    let other = runtime
        .index(
            &indexer,
            common::domain_fixture_path("external").as_path(),
            tenant.clone(),
        )
        .unwrap();
    assert!(
        runtime
            .knowledge(
                &tenant,
                &other.snapshot.repository_id,
                None,
                "",
                &command(json!({"operation":"get","id":id}))
            )
            .is_err()
    );
    assert_eq!(
        runtime
            .knowledge(
                &tenant,
                &other.snapshot.repository_id,
                None,
                "",
                &command(json!({"operation":"list","offset":0,"limit":100}))
            )
            .unwrap()["total"],
        0
    );
}
