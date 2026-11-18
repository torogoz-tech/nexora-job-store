use nexora_database_connections::DatabaseConnections;
use nexora_knowledge_store::KnowledgeStore;
use postgres::{Client, Config, NoTls};
use std::path::PathBuf;

#[test]
fn synthesis_claim_checks_authority_checksum_and_queues_rag_atomically() {
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
        "nexora-knowledge-synthesis-{}-{}",
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
        "nexora_knowledge_synthesis_{}_{}",
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

    let mut store = KnowledgeStore::connect(&settings, &namespace).unwrap();
    let job = store.claim_knowledge_synthesis().unwrap().unwrap();
    assert_eq!(job.id, "contribution-a");
    assert_eq!(job.payload, b"source payload");
    assert_eq!(job.language, "es");
    assert!(store.claim_knowledge_synthesis().unwrap().is_none());
    assert!(
        store
            .finish_knowledge_synthesis(&job, Some("Reviewed summary"))
            .unwrap()
    );

    let contribution = admin
        .query_one(
            &format!(
                "SELECT status,revision,body FROM {namespace}.knowledge_contributions WHERE id='contribution-a'"
            ),
            &[],
        )
        .unwrap();
    let status: String = contribution.get(0);
    let revision: i64 = contribution.get(1);
    let body: String = contribution.get(2);
    assert_eq!(status, "ready");
    assert_eq!(revision, 2);
    assert_eq!(body, "Reviewed summary");
    let version_count: i64 = admin
        .query_one(
            &format!(
                "SELECT count(*) FROM {namespace}.knowledge_contribution_versions WHERE contribution_id='contribution-a' AND revision=2"
            ),
            &[],
        )
        .unwrap()
        .get(0);
    let rag_status: String = admin
        .query_one(
            &format!(
                "SELECT status FROM {namespace}.knowledge_processing_jobs WHERE id='krag-contribution-a-2'"
            ),
            &[],
        )
        .unwrap()
        .get(0);
    assert_eq!(version_count, 1);
    assert_eq!(rag_status, "queued");

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
    let checksum = sha256_hex(b"source payload");
    let secret_checksum = sha256_hex(b"secret");
    admin.batch_execute(&format!(
        "CREATE SCHEMA {namespace};
         CREATE TABLE {namespace}.actors(id text PRIMARY KEY,active boolean NOT NULL);
         CREATE TABLE {namespace}.actor_roles(actor_id text NOT NULL,role text NOT NULL);
         CREATE TABLE {namespace}.actor_grants(actor_id text NOT NULL,capability text NOT NULL,project_id text);
         CREATE TABLE {namespace}.projects(id text PRIMARY KEY,archived boolean NOT NULL);
         CREATE TABLE {namespace}.installation_preferences(language text);
         CREATE TABLE {namespace}.knowledge_contributions(
             id text PRIMARY KEY,project_id text,status text NOT NULL,attempts integer NOT NULL DEFAULT 0,
             claimed_at timestamptz,created_by text NOT NULL,source_revision bigint NOT NULL,
             filename text NOT NULL,mime_type text NOT NULL,payload bytea NOT NULL,sha256 text NOT NULL,
             kind text NOT NULL,title text NOT NULL,body text NOT NULL,revision bigint NOT NULL,
             created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now()
         );
         CREATE TABLE {namespace}.knowledge_contribution_sources(
             contribution_id text NOT NULL,source_revision bigint NOT NULL,filename text NOT NULL,
             mime_type text NOT NULL,payload bytea NOT NULL,sha256 text NOT NULL,created_by text NOT NULL
         );
         CREATE TABLE {namespace}.knowledge_contribution_versions(
             contribution_id text NOT NULL,revision bigint NOT NULL,source_revision bigint NOT NULL,
             title text NOT NULL,body text NOT NULL
         );
         CREATE TABLE {namespace}.knowledge_processing_jobs(
             id text PRIMARY KEY,project_id text,source_kind text NOT NULL,resource_id text NOT NULL,
             source_revision bigint NOT NULL,content_hash text NOT NULL,status text NOT NULL,
             UNIQUE(source_kind,resource_id,source_revision,content_hash)
         );
         INSERT INTO {namespace}.actors VALUES('actor-a',true);
         INSERT INTO {namespace}.actors VALUES('actor-b',true);
         INSERT INTO {namespace}.actor_roles VALUES('actor-a','developer'),('actor-b','developer');
         INSERT INTO {namespace}.actor_grants VALUES
             ('actor-a','knowledge:contribute',NULL),('actor-a','organization-knowledge:read',NULL);
         INSERT INTO {namespace}.knowledge_contributions(
             id,status,created_by,source_revision,filename,mime_type,payload,sha256,kind,title,body,revision
         ) VALUES('contribution-a','queued','actor-a',1,'source.pdf','application/pdf',decode('736f75726365207061796c6f6164','hex'),$sha${checksum}$sha$,'document','Source title','',1);
         INSERT INTO {namespace}.knowledge_contribution_sources(
             contribution_id,source_revision,filename,mime_type,payload,sha256,created_by
         ) SELECT id,source_revision,filename,mime_type,payload,sha256,created_by
           FROM {namespace}.knowledge_contributions WHERE id='contribution-a';
         INSERT INTO {namespace}.knowledge_contributions(
             id,status,created_by,source_revision,filename,mime_type,payload,sha256,kind,title,body,revision
         ) VALUES('contribution-unauthorized','queued','actor-b',1,'private.pdf','application/pdf',decode('736563726574','hex'),$secret${secret_checksum}$secret$,'document','Private source','',1);
         INSERT INTO {namespace}.knowledge_contribution_sources(
             contribution_id,source_revision,filename,mime_type,payload,sha256,created_by
         ) SELECT id,source_revision,filename,mime_type,payload,sha256,created_by
           FROM {namespace}.knowledge_contributions WHERE id='contribution-unauthorized';"
    ))
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
