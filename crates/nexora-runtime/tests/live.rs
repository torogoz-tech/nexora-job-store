mod common;

use nexora_core::TenantId;
use nexora_indexer::Indexer;
use nexora_language::LanguageDefinition;
use nexora_runtime::{DatabaseConnections, DatabaseRuntime};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "requires isolated local PostgreSQL, Dragonfly and Neo4j; run with --ignored"]
fn real_backends_persist_reuse_isolate_and_recover_public_snapshots() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-live-{suffix}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.js"), "function hello() {}\n").unwrap();
    fs::write(dir.join("b.js"), "class World {}\n").unwrap();
    let tenant = TenantId(format!("public-live-{suffix}"));
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let language = LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let cold = runtime
        .index(&Indexer::new(language.clone()), &dir, tenant.clone())
        .unwrap();
    assert_eq!(cold.reuse.analyzed_files, 2);
    let repo = cold.snapshot.repository_id.clone();
    let original = cold.snapshot.id.clone();
    drop(runtime);
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let restored = runtime
        .snapshot(&tenant, &repo, Some(&original))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&cold.snapshot).unwrap()
    );
    let mut no_cache = config.clone();
    no_cache.redis_port = 9;
    let mut reader = DatabaseRuntime::connect(&no_cache)
        .expect("persisted snapshot reads must not require the cache service");
    assert_eq!(
        reader
            .snapshot(&tenant, &repo, Some(&original))
            .unwrap()
            .unwrap()
            .id,
        original
    );
    assert!(
        reader
            .index(&Indexer::new(language.clone()), &dir, tenant.clone())
            .is_err()
    );
    let warm = runtime
        .index(&Indexer::new(language.clone()), &dir, tenant.clone())
        .unwrap();
    assert_eq!(warm.reuse.reused_files, 2);
    assert_eq!(warm.reuse.analyzed_files, 0);
    assert_eq!(
        serde_json::to_value(&warm.snapshot).unwrap(),
        serde_json::to_value(&cold.snapshot).unwrap()
    );
    let root = &restored
        .graph
        .nodes
        .iter()
        .find(|n| n.kind == nexora_graph::NodeKind::File)
        .unwrap()
        .id;
    let traversal = runtime
        .traverse(&tenant, &repo, &original, root, 4, 1000)
        .unwrap();
    assert!(!traversal.edge_ids.is_empty());
    assert!(traversal.node_ids.len() > 1);
    assert!(
        runtime
            .snapshot(&TenantId("another-tenant".into()), &repo, Some(&original))
            .unwrap()
            .is_none()
    );
    // Corrupt one engine-owned cache entry: only that file must be recalculated.
    use sha2::{Digest, Sha256};
    let key = nexora_indexer::FileCacheKey {
        tenant: tenant.0.clone(),
        repository: repo.0.clone(),
        configuration: restored.analysis_configuration.clone().unwrap(),
        engine: nexora_indexer::ANALYSIS_ENGINE_VERSION.into(),
        path: "a.js".into(),
        content_hash: format!("{:x}", Sha256::digest(b"function hello() {}\n")),
    };
    let flags = fs::read_to_string(config.secrets_directory.join("dragonfly_flags")).unwrap();
    let password = flags
        .lines()
        .find_map(|v| v.strip_prefix("--requirepass="))
        .unwrap();
    let client = redis::Client::open(redis::ConnectionInfo {
        addr: redis::ConnectionAddr::Tcp(config.host.clone(), config.redis_port),
        redis: redis::RedisConnectionInfo {
            password: Some(password.into()),
            ..Default::default()
        },
    })
    .unwrap();
    let mut connection = client.get_connection().unwrap();
    let cache_key = nexora_runtime::DragonflyCache::key(&key).unwrap();
    let ttl: i64 = redis::cmd("TTL")
        .arg(&cache_key)
        .query(&mut connection)
        .unwrap();
    assert!((1..=86_400).contains(&ttl));
    let _: String = redis::cmd("SET")
        .arg(&cache_key)
        .arg("{broken JSON")
        .arg("EX")
        .arg(600)
        .query(&mut connection)
        .unwrap();
    let repaired = runtime
        .index(&Indexer::new(language.clone()), &dir, tenant.clone())
        .unwrap();
    assert_eq!(repaired.reuse.analyzed_files, 1);
    assert_eq!(repaired.reuse.reused_files, 1);
    assert_eq!(
        serde_json::to_value(&repaired.snapshot).unwrap(),
        serde_json::to_value(&restored).unwrap()
    );
    fs::write(dir.join("b.js"), "class Changed {}\n").unwrap();
    let changed = runtime
        .index(&Indexer::new(language.clone()), &dir, tenant.clone())
        .unwrap();
    assert_eq!(changed.reuse.reused_files, 1);
    assert_eq!(changed.reuse.analyzed_files, 1);
    assert_ne!(changed.snapshot.id, original);
    let full = Indexer::new(language.clone())
        .index(
            &dir,
            tenant.clone(),
            &mut nexora_indexer::MemoryStore::default(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(full).unwrap(),
        serde_json::to_value(&changed.snapshot).unwrap()
    );
    runtime.publish(&cold.snapshot, false).unwrap();
    assert_eq!(
        runtime.snapshot(&tenant, &repo, None).unwrap().unwrap().id,
        changed.snapshot.id
    );
    let mut corrupted = changed.snapshot.clone();
    corrupted.graph.nodes[0].name.push_str("-tampered");
    assert!(runtime.publish(&corrupted, false).is_err());
    fs::write(dir.join("b.js"), "class Pending {}\n").unwrap();
    let mut bad = config.clone();
    bad.neo4j_http_port = 9;
    let mut disconnected = DatabaseRuntime::connect(&bad).unwrap();
    assert!(
        disconnected
            .index(&Indexer::new(language.clone()), &dir, tenant.clone())
            .is_err()
    );
    let current = runtime.snapshot(&tenant, &repo, None).unwrap().unwrap();
    assert_eq!(current.id, changed.snapshot.id);
    let recovered = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap();
    assert_eq!(recovered.reuse.analyzed_files, 0);
    assert_eq!(recovered.reuse.reused_files, 2);
    assert_eq!(
        runtime.snapshot(&tenant, &repo, None).unwrap().unwrap().id,
        recovered.snapshot.id
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires local databases and permissions to install a tenant-scoped failure trigger"]
fn neo4j_ready_before_sql_commit_failure_remains_unpublished_and_retries() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-commit-failure-{suffix}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.js"), "class Before {}\n").unwrap();
    let tenant = TenantId(format!("public-failure-{suffix}"));
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let language = LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap();
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let old = runtime
        .index(&Indexer::new(language.clone()), &dir, tenant.clone())
        .unwrap();
    let password = fs::read_to_string(config.secrets_directory.join("postgres_password")).unwrap();
    let mut pg = postgres::Config::new()
        .host(&config.host)
        .port(config.postgres_port)
        .user(&config.postgres_user)
        .password(password.trim())
        .dbname(&config.postgres_database)
        .connect(postgres::NoTls)
        .unwrap();
    let trigger = format!("fail_ready_{suffix}");
    // Identifiers and tenant are generated exclusively from a numeric test nonce.
    pg.batch_execute(&format!("CREATE FUNCTION nexora_runtime.{trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.tenant_id='{}' AND NEW.state='ready' THEN RAISE EXCEPTION 'public fixture commit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER {trigger} BEFORE UPDATE ON nexora_runtime.snapshots FOR EACH ROW EXECUTE FUNCTION nexora_runtime.{trigger}();",tenant.0)).unwrap();
    fs::write(dir.join("a.js"), "class After {}\n").unwrap();
    let failed = runtime.index(&Indexer::new(language.clone()), &dir, tenant.clone());
    pg.batch_execute(&format!("DROP TRIGGER {trigger} ON nexora_runtime.snapshots; DROP FUNCTION nexora_runtime.{trigger}();")).unwrap();
    assert!(failed.is_err());
    let pending = Indexer::new(language.clone())
        .index(
            &dir,
            tenant.clone(),
            &mut nexora_indexer::MemoryStore::default(),
        )
        .unwrap();
    assert!(
        runtime
            .snapshot(&tenant, &old.snapshot.repository_id, Some(&pending.id))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        runtime
            .snapshot(&tenant, &old.snapshot.repository_id, None)
            .unwrap()
            .unwrap()
            .id,
        old.snapshot.id
    );
    let auth = fs::read_to_string(config.secrets_directory.join("neo4j_auth")).unwrap();
    let (user, password) = auth.trim().split_once('/').unwrap();
    let scope =
        nexora_runtime::Neo4jProjection::scope(&tenant, &old.snapshot.repository_id, &pending.id)
            .unwrap();
    let mut response=ureq::post(format!("http://{}:{}/db/neo4j/tx/commit",config.host,config.neo4j_http_port)).header("Authorization",&format!("Basic {}",STANDARD.encode(format!("{user}:{password}")))).send_json(serde_json::json!({"statements":[{"statement":"MATCH (s:NexoraSnapshot {scope:$scope}) RETURN s.state","parameters":{"scope":scope}}]})).unwrap();
    let value: serde_json::Value = response.body_mut().read_json().unwrap();
    assert_eq!(value["results"][0]["data"][0]["row"][0], "ready");
    let recovered = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .unwrap();
    assert_eq!(recovered.reuse.analyzed_files, 0);
    assert_eq!(
        runtime
            .snapshot(&tenant, &old.snapshot.repository_id, None)
            .unwrap()
            .unwrap()
            .id,
        recovered.snapshot.id
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires local services; verifies lossless source retention even on lexical errors"]
fn source_with_zero_byte_roundtrips_without_changing_diagnostics() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-lossless-{nonce}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.js"), "class Kept {}\n\0").unwrap();
    let config = DatabaseConnections::load(common::connections_config_path().as_path()).unwrap();
    let language = LanguageDefinition::load(common::javascript_profile_path().as_path()).unwrap();
    let tenant = TenantId(format!("public-lossless-{nonce}"));
    let expected = Indexer::new(language.clone())
        .index(
            &dir,
            tenant.clone(),
            &mut nexora_indexer::MemoryStore::default(),
        )
        .unwrap();
    assert!(expected.sources["a.js"].contains('\0'));
    let mut runtime = DatabaseRuntime::connect(&config).unwrap();
    let result = runtime
        .index(&Indexer::new(language), &dir, tenant.clone())
        .expect("all source accepted by SourceReader must remain persistable");
    let stored = runtime
        .snapshot(
            &tenant,
            &result.snapshot.repository_id,
            Some(&result.snapshot.id),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(stored).unwrap()
    );
    fs::remove_dir_all(dir).unwrap();
}
