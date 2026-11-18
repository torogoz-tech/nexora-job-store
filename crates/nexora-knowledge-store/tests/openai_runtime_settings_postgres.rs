use nexora_database_connections::DatabaseConnections;
use nexora_knowledge_store::KnowledgeStore;
use postgres::{Config, NoTls};
use serde_json::json;
use std::path::PathBuf;

const NAMESPACE: &str = "nexora_knowledge_runtime_fixture";

#[test]
fn reads_api_managed_openai_settings_and_decrypts_the_bound_secret() {
    let Some(host) = std::env::var_os("NEXORA_TEST_POSTGRES_HOST") else {
        assert!(
            std::env::var_os("NEXORA_TEST_POSTGRES_REQUIRED").is_none(),
            "PostgreSQL integration test service is required but not configured"
        );
        eprintln!("PostgreSQL integration test skipped: test service is not configured");
        return;
    };
    let password = std::env::var("NEXORA_TEST_POSTGRES_PASSWORD").unwrap();
    let port = std::env::var("NEXORA_TEST_POSTGRES_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let user = std::env::var("NEXORA_TEST_POSTGRES_USER").unwrap();
    let database = std::env::var("NEXORA_TEST_POSTGRES_DATABASE").unwrap();
    let root = std::env::temp_dir().join(format!(
        "nexora-openai-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    write_secret(&root.join("postgres_password"), password.as_bytes());
    let master_key = [7_u8; 32];
    write_secret(
        &root.join(format!("{NAMESPACE}_master_key_v1")),
        &master_key,
    );

    let settings = DatabaseConnections {
        schema_version: 1,
        host: host.to_string_lossy().into_owned(),
        postgres_port: port,
        redis_port: 6379,
        neo4j_http_port: 7474,
        postgres_database: database.clone(),
        postgres_user: user.clone(),
        secrets_directory: PathBuf::from(&root),
        allow_remote_host: false,
    };
    let mut admin = postgres_config(&host.to_string_lossy(), port, &user, &password, &database)
        .connect(NoTls)
        .unwrap();
    admin
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {NAMESPACE} CASCADE;
             CREATE SCHEMA {NAMESPACE};
             CREATE TABLE {NAMESPACE}.platform_crypto_state(
                 singleton boolean PRIMARY KEY DEFAULT true,
                 key_version integer NOT NULL,
                 key_id text NOT NULL
             );
             CREATE TABLE {NAMESPACE}.installation_services(
                 kind text PRIMARY KEY,
                 settings jsonb NOT NULL,
                 secret jsonb,
                 secret_revision bigint NOT NULL,
                 managed boolean NOT NULL
             );"
        ))
        .unwrap();

    let key_id = sha256_hex(&master_key);
    admin
        .execute(
            &format!(
                "INSERT INTO {NAMESPACE}.platform_crypto_state(singleton,key_version,key_id) VALUES(true,1,$1)"
            ),
            &[&key_id],
        )
        .unwrap();
    let openai_settings = json!({
        "endpoint": "https://api.openai.com/v1",
        "model": "gpt-4o",
        "embedding_model": "text-embedding-3-small",
        "effort": "medium",
        "enabled": true
    });
    let secret_envelope = json!({
        "key_version": 1,
        "nonce": [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
        "ciphertext": [83, 156, 47, 202, 13, 106, 53, 112, 127, 135, 104, 63, 229, 138, 6, 171, 15, 99, 83, 15, 224, 63, 140, 162, 80, 40, 60, 173, 193, 11, 103, 158, 98, 207, 239, 62]
    });
    admin
        .execute(
            &format!(
                "INSERT INTO {NAMESPACE}.installation_services(kind,settings,secret,secret_revision,managed) VALUES('openai',$1,$2,1,true)"
            ),
            &[&openai_settings, &secret_envelope],
        )
        .unwrap();

    let mut store = KnowledgeStore::connect(&settings, NAMESPACE).unwrap();
    let runtime = store.openai_service_runtime_settings().unwrap().unwrap();
    assert_eq!(runtime.settings, openai_settings);
    assert_eq!(runtime.secret.unwrap().as_slice(), b"fixture-openai-token");

    admin
        .batch_execute(&format!("DROP SCHEMA {NAMESPACE} CASCADE"))
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn write_secret(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn postgres_config(host: &str, port: u16, user: &str, password: &str, database: &str) -> Config {
    let mut config = Config::new();
    config
        .host(host)
        .port(port)
        .user(user)
        .password(password)
        .dbname(database);
    config
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
