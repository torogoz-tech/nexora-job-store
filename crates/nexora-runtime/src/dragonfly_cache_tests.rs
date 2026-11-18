use crate::{DatabaseConnections, DragonflyCache, connection_secrets::ConnectionSecrets};
use nexora_indexer::{CacheFragment, FileCacheKey, Indexer};
use nexora_language::LanguageDefinition;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "requires isolated local Dragonfly; run runtime tests with --ignored"]
fn rejected_nx_insert_repairs_corrupt_existing_value_instead_of_claiming_success() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = DatabaseConnections::load(crate::test_environment::connections_config_path().as_path())
        .unwrap();
    let secrets = ConnectionSecrets::load(&c).unwrap();
    let mut adapter = DragonflyCache::connect(&c, &secrets).unwrap();
    let client = redis::Client::open(redis::ConnectionInfo {
        addr: redis::ConnectionAddr::Tcp(c.host.clone(), c.redis_port),
        redis: redis::RedisConnectionInfo {
            password: Some(secrets.redis.clone()),
            ..Default::default()
        },
    })
    .unwrap();
    let mut connection = client.get_connection().unwrap();
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("nexora-cache-nx-{nonce}"));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.js");
    fs::write(&file, "class Original {}\n").unwrap();
    let graph = Indexer::new(
        LanguageDefinition::load(crate::test_environment::javascript_profile_path().as_path())
            .unwrap(),
    )
    .analyze(&file)
    .unwrap()
    .graph;
    let key = FileCacheKey {
        tenant: format!("public-nx-{nonce}"),
        repository: "fixture".into(),
        configuration: "fixture".into(),
        engine: nexora_indexer::ANALYSIS_ENGINE_VERSION.into(),
        path: "a.js".into(),
        content_hash: "fixture".into(),
    };
    let fragment = CacheFragment::new(key.clone(), graph, 100).unwrap();
    let name = DragonflyCache::key(&key).unwrap();
    let _: String = redis::cmd("SET")
        .arg(&name)
        .arg("{invalid JSON")
        .arg("EX")
        .arg(600)
        .query(&mut connection)
        .unwrap();
    assert!(adapter.write(&key, &fragment).unwrap());
    let bytes: Vec<u8> = redis::cmd("GET").arg(&name).query(&mut connection).unwrap();
    let stored: CacheFragment = serde_json::from_slice(&bytes)
        .expect("write reported success without storing a valid fragment");
    stored.validate_for(&key).unwrap();
    assert!(stored.same_analysis(&fragment).unwrap());
    let _: usize = redis::cmd("DEL").arg(name).query(&mut connection).unwrap();
    fs::remove_dir_all(dir).unwrap();
}
