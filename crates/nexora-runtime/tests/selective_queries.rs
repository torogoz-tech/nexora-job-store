mod common;

use nexora_core::{NodeId, TenantId};
use nexora_indexer::Indexer;
use nexora_language::LanguageDefinition;
use nexora_runtime::{DatabaseConnections, DatabaseRuntime, KnowledgeQuery};
use serde_json::json;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn query_bounds_are_checked_before_storage_access() {
    assert!(
        KnowledgeQuery::FindSymbol {
            name: "x".into(),
            offset: 0,
            limit: 0
        }
        .validate()
        .is_err()
    );
    assert!(
        KnowledgeQuery::Context {
            path: "a.js".into(),
            start_line: 0,
            line_count: 5
        }
        .validate()
        .is_err()
    );
    assert!(
        KnowledgeQuery::Search {
            query: "".into(),
            offset: 0,
            limit: 10
        }
        .validate()
        .is_err()
    );
    assert!(
        KnowledgeQuery::Traverse {
            node: NodeId("n".into()),
            depth: 33,
            limit: 10
        }
        .validate()
        .is_err()
    );
    assert!(
        KnowledgeQuery::Search {
            query: "\0%_\\".into(),
            offset: 0,
            limit: 100
        }
        .validate()
        .is_ok()
    );
}

#[test]
#[ignore = "requires isolated real databases; public synthetic source only"]
fn selective_queries_preserve_bytes_versions_scope_and_cross_process_reuse() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-selective-{suffix}"));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("a.js"),
        "function hello() {}\r\n// 🐦 literal % _ \\\r\n// nul \0 preserved\n// replacement �\n",
    )
    .unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("public-selective-{suffix}"));
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let first = runtime.index(&indexer, &dir, tenant.clone()).unwrap();
    let repo = first.snapshot.repository_id.clone();
    let original = first.snapshot.id.clone();
    let query = KnowledgeQuery::FindSymbol {
        name: "hello".into(),
        offset: 0,
        limit: 100,
    };
    let found = runtime
        .query(&tenant, &repo, Some(&original), &query)
        .unwrap();
    assert!(!runtime.query_metrics().unwrap().cache_hit);
    assert_eq!(runtime.query_metrics().unwrap().backend, "neo4j");
    assert_eq!(found["items"][0]["name"], "hello");
    let again = runtime
        .query(&tenant, &repo, Some(&original), &query)
        .unwrap();
    assert_eq!(found, again);
    assert!(runtime.query_metrics().unwrap().cache_hit);
    drop(runtime);
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    assert_eq!(
        runtime
            .query(&tenant, &repo, Some(&original), &query)
            .unwrap(),
        found
    );
    assert!(runtime.query_metrics().unwrap().cache_hit);
    for term in ["%", "_", "\\", "\0", "🐦"] {
        let value = runtime
            .query(
                &tenant,
                &repo,
                Some(&original),
                &KnowledgeQuery::Search {
                    query: term.into(),
                    offset: 0,
                    limit: 100,
                },
            )
            .unwrap();
        assert_eq!(value["total"], 1, "{term:?}");
    }
    let nul_page = runtime
        .query(
            &tenant,
            &repo,
            Some(&original),
            &KnowledgeQuery::Search {
                query: "\0".into(),
                offset: 1,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(nul_page["total"], 1);
    assert_eq!(nul_page["items"], json!([]));
    let replacement = runtime
        .query(
            &tenant,
            &repo,
            Some(&original),
            &KnowledgeQuery::Search {
                query: "�".into(),
                offset: 0,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(replacement["total"], 1);
    assert_eq!(replacement["items"][0]["line"], 4);
    let context = runtime
        .query(
            &tenant,
            &repo,
            Some(&original),
            &KnowledgeQuery::Context {
                path: "a.js".into(),
                start_line: 3,
                line_count: 1,
            },
        )
        .unwrap();
    assert_eq!(context["items"][0]["text"], "// nul \0 preserved");
    assert_eq!(context["total_lines"], 4);
    assert_eq!(context["next_line"], json!(4));
    assert!(
        runtime
            .query(&TenantId("other".into()), &repo, Some(&original), &query)
            .is_err()
    );
    fs::write(dir.join("a.js"), "function changed() {}\n").unwrap();
    let changed = runtime.index(&indexer, &dir, tenant.clone()).unwrap();
    assert_ne!(changed.snapshot.id, original);
    assert_eq!(
        runtime.query(&tenant, &repo, None, &query).unwrap()["items"],
        json!([])
    );
    assert_eq!(
        runtime
            .query(&tenant, &repo, Some(&original), &query)
            .unwrap(),
        found
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires real Neo4j; validates arbitrary long source names without truncation"]
fn graph_projection_and_traversal_preserve_names_larger_than_native_index_keys() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-long-name-{suffix}"));
    fs::create_dir(&dir).unwrap();
    let name = "a".repeat(10_000);
    fs::write(dir.join("a.js"), format!("function {name}() {{}}\n")).unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let indexer = Indexer::new(
        LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap(),
    );
    let tenant = TenantId(format!("public-long-name-{suffix}"));
    let result = runtime.index(&indexer, &dir, tenant.clone()).unwrap();
    let value = runtime
        .query(
            &tenant,
            &result.snapshot.repository_id,
            Some(&result.snapshot.id),
            &KnowledgeQuery::Traverse {
                node: NodeId("file:a.js".into()),
                depth: 3,
                limit: 10,
            },
        )
        .unwrap();
    assert!(
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == name)
    );
    fs::remove_dir_all(dir).unwrap();
}
