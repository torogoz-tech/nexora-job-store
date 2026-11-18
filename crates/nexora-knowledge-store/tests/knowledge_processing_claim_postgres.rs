use nexora_database_connections::DatabaseConnections;
use nexora_knowledge_store::{KnowledgeChunkRecord, KnowledgeStore};
use postgres::{Client, Config, NoTls};
use std::{path::PathBuf, sync::Barrier};

#[test]
fn concurrent_claims_assign_one_approved_job_to_only_one_worker() {
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
        "nexora-knowledge-store-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let secret_path = root.join("postgres_password");
    std::fs::write(&secret_path, &password).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

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
    let namespace = format!(
        "nexora_knowledge_it_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut admin = postgres_config(&host.to_string_lossy(), port, &user, &password, &database)
        .connect(NoTls)
        .unwrap();
    create_schema(&mut admin, &namespace).unwrap();

    let barrier = std::sync::Arc::new(Barrier::new(2));
    let claims = [0, 1].map(|_| {
        let settings = settings.clone();
        let namespace = namespace.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let mut store = KnowledgeStore::connect(&settings, &namespace).unwrap();
            barrier.wait();
            store.claim_knowledge_processing().unwrap()
        })
    });
    let mut claimed = claims
        .into_iter()
        .flat_map(|claim| claim.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, "job-approved");
    assert!(!claimed[0].lease_token.is_empty());

    let first_lease = claimed.pop().unwrap();
    admin
        .execute(
            &format!(
                "UPDATE {namespace}.knowledge_processing_jobs SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1"
            ),
            &[&first_lease.id],
        )
        .unwrap();
    let mut store = KnowledgeStore::connect(&settings, &namespace).unwrap();
    let second_lease = store.claim_knowledge_processing().unwrap().unwrap();
    assert_eq!(second_lease.id, first_lease.id);
    let source = store.knowledge_processing_source(&second_lease).unwrap();
    assert_eq!(source.title, "Reviewed source");
    assert_eq!(source.body, "Source body");
    assert_eq!(source.kind, "document");
    let structure = serde_json::json!({ "summary": "Reviewed source summary" });
    let chunks = [KnowledgeChunkRecord {
        id: "chunk-a".to_owned(),
        sequence: 0,
        title: "Reviewed source".to_owned(),
        content: "Source body".to_owned(),
        content_hash: "chunk-hash-a".to_owned(),
    }];
    assert!(
        store
            .finish_knowledge_processing(&first_lease, &structure, &chunks)
            .is_err()
    );
    store
        .finish_knowledge_processing(&second_lease, &structure, &chunks)
        .unwrap();
    assert!(
        !store
            .fail_knowledge_processing(&first_lease, "stale worker")
            .unwrap()
    );
    assert!(
        store
            .fail_knowledge_processing(&second_lease, "current worker")
            .unwrap()
    );
    let state: String = admin
        .query_one(
            &format!("SELECT status FROM {namespace}.knowledge_processing_jobs WHERE id=$1"),
            &[&second_lease.id],
        )
        .unwrap()
        .get(0);
    assert_eq!(state, "failed");

    admin
        .batch_execute(&format!("DROP SCHEMA {namespace} CASCADE"))
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
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

fn create_schema(admin: &mut Client, namespace: &str) -> Result<(), postgres::Error> {
    admin.batch_execute(&format!(
        "CREATE SCHEMA {namespace};
         CREATE TABLE {namespace}.knowledge_processing_jobs (
             id text PRIMARY KEY, project_id text, source_kind text NOT NULL,
             resource_id text NOT NULL, source_revision bigint NOT NULL, content_hash text NOT NULL,
             status text NOT NULL, attempts integer NOT NULL DEFAULT 0, claimed_at timestamptz,
             last_error text, created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
             updated_at timestamptz NOT NULL DEFAULT clock_timestamp(), lease_owner text,
             lease_token_digest bytea, lease_until timestamptz
         );
         CREATE TABLE {namespace}.knowledge_reviews (
             id text PRIMARY KEY, kind text NOT NULL, resource_id text NOT NULL,
             resource_revision bigint NOT NULL, status text NOT NULL
         );
         CREATE TABLE {namespace}.knowledge_contributions (
             id text NOT NULL, revision bigint NOT NULL, title text NOT NULL,
             body text NOT NULL, kind text NOT NULL, PRIMARY KEY(id,revision)
         );
         CREATE TABLE {namespace}.knowledge_structures (
             job_id text PRIMARY KEY, source_kind text NOT NULL, resource_id text NOT NULL,
             source_revision bigint NOT NULL, schema_version text NOT NULL,
             structure jsonb NOT NULL, structure_hash text NOT NULL,
             validation_status text NOT NULL
         );
         CREATE TABLE {namespace}.knowledge_chunks (
             id text PRIMARY KEY, job_id text NOT NULL, project_id text,
             source_kind text NOT NULL, resource_id text NOT NULL, source_revision bigint NOT NULL,
             sequence integer NOT NULL, title text NOT NULL, content text NOT NULL,
             content_hash text NOT NULL, published boolean NOT NULL DEFAULT false,
             UNIQUE(job_id,sequence)
         );
         INSERT INTO {namespace}.knowledge_processing_jobs
             (id,source_kind,resource_id,source_revision,content_hash,status)
         VALUES ('job-approved','document','resource-a',1,'hash-a','queued'),
                ('job-unreviewed','document','resource-b',1,'hash-b','queued');
         INSERT INTO {namespace}.knowledge_reviews
             (id,kind,resource_id,resource_revision,status)
         VALUES ('review-a','contribution','resource-a',1,'approved');
         INSERT INTO {namespace}.knowledge_contributions(id,revision,title,body,kind)
         VALUES ('resource-a',1,'Reviewed source','Source body','document');"
    ))
}
