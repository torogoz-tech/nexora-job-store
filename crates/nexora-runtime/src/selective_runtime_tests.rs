use super::*;
use crate::KnowledgeQuery;
use nexora_language::LanguageDefinition;
use serde_json::json;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "requires real databases; proves ordinary reads cannot select canonical payload"]
fn queries_work_when_canonical_payload_select_is_denied() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-no-payload-{suffix}"));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("a.js"),
        "function hello() {}\n// source evidence\n",
    )
    .unwrap();
    let config =
        DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
            .unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let tenant = TenantId(format!("public-no-payload-{suffix}"));
    let language =
        LanguageDefinition::load(crate::test_environment::javascript_profile_path().as_path())
            .unwrap();
    let result = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap();
    let repo = result.snapshot.repository_id.clone();
    let id = result.snapshot.id.clone();
    let role = format!("nexora_q_{suffix}");
    runtime.postgres.client().batch_execute(&format!("CREATE ROLE {role} NOLOGIN; GRANT USAGE ON SCHEMA nexora_runtime TO {role}; GRANT SELECT(tenant_id,repository_id,current_snapshot_id) ON nexora_runtime.repositories TO {role}; GRANT SELECT(tenant_id,repository_id,snapshot_id,payload_hash,state) ON nexora_runtime.snapshots TO {role}; GRANT SELECT ON nexora_runtime.query_snapshots,nexora_runtime.query_files,nexora_runtime.query_lines TO {role}; SET ROLE {role};")).unwrap();
    let assertions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(
            runtime.snapshot(&tenant, &repo, Some(&id)).is_err(),
            "full payload must be inaccessible to the test role"
        );
        let found = runtime
            .query(
                &tenant,
                &repo,
                Some(&id),
                &KnowledgeQuery::FindSymbol {
                    name: "hello".into(),
                    offset: 0,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(found["items"][0]["name"], "hello");
        assert!(!runtime.query_metrics().unwrap().cache_hit);
        assert_eq!(runtime.query_metrics().unwrap().backend, "neo4j");
        let context = runtime
            .query(
                &tenant,
                &repo,
                Some(&id),
                &KnowledgeQuery::Context {
                    path: "a.js".into(),
                    start_line: 2,
                    line_count: 1,
                },
            )
            .unwrap();
        assert_eq!(context["items"][0]["text"], "// source evidence");
        let search = runtime
            .query(
                &tenant,
                &repo,
                Some(&id),
                &KnowledgeQuery::Search {
                    query: "evidence".into(),
                    offset: 0,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(search["total"], 1);
        let graph = runtime
            .query(
                &tenant,
                &repo,
                Some(&id),
                &KnowledgeQuery::Traverse {
                    node: NodeId("file:a.js".into()),
                    depth: 3,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(
            runtime
                .query(
                    &tenant,
                    &repo,
                    Some(&id),
                    &KnowledgeQuery::Overview {
                        offset: 10,
                        limit: 10
                    }
                )
                .unwrap()["items"],
            json!([])
        );
    }));
    runtime
        .postgres
        .client()
        .batch_execute(&format!(
            "RESET ROLE; DROP OWNED BY {role}; DROP ROLE {role};"
        ))
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    if let Err(e) = assertions {
        std::panic::resume_unwind(e);
    }
}

#[test]
#[ignore = "requires real databases; validates selected metadata and query-cache corruption"]
fn selected_metadata_and_query_cache_corruption_are_detected() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-query-corrupt-{suffix}"));
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.js"), "function hello() {}\n// second line\n").unwrap();
    let config =
        DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
            .unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let tenant = TenantId(format!("public-query-corrupt-{suffix}"));
    let language =
        LanguageDefinition::load(crate::test_environment::javascript_profile_path().as_path())
            .unwrap();
    let indexed = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap();
    let repo = &indexed.snapshot.repository_id;
    let id = &indexed.snapshot.id;
    runtime.postgres.client().execute("UPDATE nexora_runtime.query_files SET total_lines=total_lines+1 WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&tenant.0,&repo.0,&id.0]).unwrap();
    assert!(
        runtime
            .query(
                &tenant,
                repo,
                Some(id),
                &KnowledgeQuery::Context {
                    path: "a.js".into(),
                    start_line: 1,
                    line_count: 1
                }
            )
            .is_err(),
        "tampered total_lines must not produce a trusted response"
    );
    runtime.postgres.client().execute("UPDATE nexora_runtime.query_files SET total_lines=total_lines-1 WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&tenant.0,&repo.0,&id.0]).unwrap();
    let q = KnowledgeQuery::FindSymbol {
        name: "hello".into(),
        offset: 0,
        limit: 10,
    };
    let expected = runtime.query(&tenant, repo, Some(id), &q).unwrap();
    let head = runtime.query_header(&tenant, repo, Some(id)).unwrap();
    let key = crate::QueryResultCache::key(&head, &q).unwrap();
    {
        let mut cache = runtime.cache.borrow_mut();
        let bytes: Vec<u8> = redis::cmd("GET")
            .arg(&key)
            .query(cache.connection().unwrap())
            .unwrap();
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        record["data"]["items"][0]["name"] = json!("tampered");
        let _: String = redis::cmd("SET")
            .arg(&key)
            .arg(serde_json::to_vec(&record).unwrap())
            .arg("EX")
            .arg(600)
            .query(cache.connection().unwrap())
            .unwrap();
    }
    assert_eq!(
        runtime.query(&tenant, repo, Some(id), &q).unwrap(),
        expected
    );
    assert!(!runtime.query_metrics().unwrap().cache_hit);
    let mut down = config.clone();
    down.redis_port = 9;
    let mut reader = DatabaseRuntime::connect(&down).unwrap();
    assert_eq!(reader.query(&tenant, repo, Some(id), &q).unwrap(), expected);
    assert!(!reader.query_metrics().unwrap().cache_available);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires real Dragonfly; disconnects only this test's own cache connection"]
fn query_cache_recovers_after_its_connection_is_closed() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-cache-reconnect-{suffix}"));
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.js"), "function hello() {}\n").unwrap();
    let config =
        DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
            .unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let tenant = TenantId(format!("public-cache-reconnect-{suffix}"));
    let language =
        LanguageDefinition::load(crate::test_environment::javascript_profile_path().as_path())
            .unwrap();
    let s = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap()
        .snapshot;
    let q = crate::KnowledgeQuery::FindSymbol {
        name: "hello".into(),
        offset: 0,
        limit: 10,
    };
    let expected = runtime
        .query(&tenant, &s.repository_id, Some(&s.id), &q)
        .unwrap();
    let connection_id: i64 = redis::cmd("CLIENT")
        .arg("ID")
        .query(runtime.cache.borrow_mut().connection().unwrap())
        .unwrap();
    let mut killer =
        DragonflyCache::connect(&config, &ConnectionSecrets::load(&config).unwrap()).unwrap();
    let killed: i64 = redis::cmd("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(connection_id)
        .query(killer.connection().unwrap())
        .unwrap();
    assert_eq!(killed, 1);
    assert_eq!(
        runtime
            .query(&tenant, &s.repository_id, Some(&s.id), &q)
            .unwrap(),
        expected
    );
    assert_eq!(
        runtime
            .query(&tenant, &s.repository_id, Some(&s.id), &q)
            .unwrap(),
        expected
    );
    assert!(
        runtime.query_metrics().unwrap().cache_hit,
        "the next query must reconnect and reuse the durable cache"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires real databases; source evidence location is part of the verified record"]
fn source_line_location_cannot_be_changed_without_detection() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-line-location-{suffix}"));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("a.js"),
        "function hello() {}\n// unique-location-evidence\n",
    )
    .unwrap();
    let config =
        DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
            .unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let tenant = TenantId(format!("public-line-location-{suffix}"));
    let language =
        LanguageDefinition::load(crate::test_environment::javascript_profile_path().as_path())
            .unwrap();
    let s = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap()
        .snapshot;
    runtime.postgres.client().execute("UPDATE nexora_runtime.query_lines SET line_number=20 WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND line_number=2",&[&tenant.0,&s.repository_id.0,&s.id.0]).unwrap();
    assert!(
        runtime
            .query(
                &tenant,
                &s.repository_id,
                Some(&s.id),
                &crate::KnowledgeQuery::Search {
                    query: "unique-location-evidence".into(),
                    offset: 0,
                    limit: 10
                }
            )
            .is_err(),
        "a checksum-valid line relocated to false evidence must be rejected"
    );
    fs::remove_dir_all(dir).unwrap();
}
