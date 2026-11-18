use nexora_database_connections::DatabaseConnections;
use nexora_knowledge_store::KnowledgeStore;
use postgres::{Client, Config, NoTls};
use std::path::PathBuf;

#[test]
fn rating_refresh_consumes_dirty_state_and_is_idempotent() {
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
        "nexora-rating-refresh-{}-{}",
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
        "nexora_rating_refresh_{}_{}",
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
    assert_eq!(store.refresh_developer_ratings().unwrap(), 1);
    assert_eq!(store.refresh_developer_ratings().unwrap(), 0);
    let row = admin
        .query_one(
            &format!(
                "SELECT count(*),(array_agg(reason))[1],(SELECT count(*) FROM {namespace}.developer_rating_dirty),(SELECT count(*) FROM {namespace}.developer_rating_closures) FROM {namespace}.developer_rating_entries"
            ),
            &[],
        )
        .unwrap();
    let entry_count: i64 = row.get(0);
    let reason: String = row.get(1);
    let dirty_count: i64 = row.get(2);
    let closure_count: i64 = row.get(3);
    assert_eq!(entry_count, 1);
    assert_eq!(reason, "no_push");
    assert_eq!(dirty_count, 0);
    assert_eq!(closure_count, 1);

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
         CREATE TABLE {namespace}.labor_calendar_settings(
             singleton boolean PRIMARY KEY,timezone text NOT NULL,starts_on date NOT NULL,
             revision bigint NOT NULL
         );
         CREATE TABLE {namespace}.actors(
             id text PRIMARY KEY,active boolean NOT NULL,kind text NOT NULL,created_at timestamptz NOT NULL
         );
         CREATE TABLE {namespace}.actor_roles(
             actor_id text NOT NULL,role text NOT NULL,rating_started_at timestamptz NOT NULL
         );
         CREATE TABLE {namespace}.developer_rating_dirty(actor_id text PRIMARY KEY);
         CREATE TABLE {namespace}.developer_rating_closures(
             actor_id text PRIMARY KEY,last_closed date NOT NULL,calendar_revision bigint NOT NULL
         );
         CREATE TABLE {namespace}.developer_push_activity(
             actor_id text NOT NULL,provider text NOT NULL,scope_id text NOT NULL,
             delivery_id text NOT NULL,project_id text,occurred_at timestamptz NOT NULL,
             processed boolean NOT NULL DEFAULT false
         );
         CREATE TABLE {namespace}.git_commit_authorship(
             actor_id text NOT NULL,occurred_at timestamptz NOT NULL
         );
         CREATE TABLE {namespace}.developer_rating_entries(
             id text NOT NULL,actor_id text NOT NULL,date date NOT NULL,source text NOT NULL,
             score double precision NOT NULL,reason text NOT NULL
         );
         CREATE UNIQUE INDEX developer_rating_activity_unique ON {namespace}.developer_rating_entries(actor_id,date) WHERE source='activity';
         CREATE FUNCTION {namespace}.developer_rating_excluded(text,date) RETURNS boolean
             LANGUAGE sql AS 'SELECT false';
         INSERT INTO {namespace}.labor_calendar_settings VALUES(true,'UTC',current_date-1,1);
         INSERT INTO {namespace}.actors VALUES('member-a',true,'human',(current_date-1)::timestamp);
         INSERT INTO {namespace}.actor_roles VALUES('member-a','developer',(current_date-1)::timestamp);
         INSERT INTO {namespace}.developer_rating_dirty VALUES('member-a');"
    ))
}
