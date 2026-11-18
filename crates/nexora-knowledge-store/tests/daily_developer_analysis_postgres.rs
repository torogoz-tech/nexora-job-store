use nexora_database_connections::DatabaseConnections;
use nexora_knowledge_store::KnowledgeStore;
use postgres::{Client, Config, NoTls};
use serde_json::json;
use std::path::PathBuf;

#[test]
fn daily_analysis_is_scheduled_claimed_with_evidence_and_fenced_on_finish() {
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
        "nexora-daily-analysis-{}-{}",
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
        "nexora_daily_analysis_{}_{}",
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
    assert_eq!(store.schedule_daily_developer_analyses().unwrap(), 1);
    let job = store.claim_daily_developer_analysis().unwrap().unwrap();
    assert_eq!(job.member_id, "member-a");
    assert_eq!(job.language, "en");
    assert_eq!(job.evidence["items"].as_array().unwrap().len(), 1);
    assert!(!job.lease_token.is_empty());
    assert!(
        store
            .finish_daily_developer_analysis(&job, Some(&json!({"summary":"Reviewed day"})))
            .unwrap()
    );
    assert!(
        !store
            .finish_daily_developer_analysis(&job, Some(&json!({"summary":"Stale result"})))
            .unwrap()
    );

    let state = admin
        .query_one(
            &format!(
                "SELECT status,report->>'summary',lease_token FROM {namespace}.daily_developer_analyses WHERE actor_id='member-a'"
            ),
            &[],
        )
        .unwrap();
    let status: String = state.get(0);
    let summary: String = state.get(1);
    let lease: Option<String> = state.get(2);
    assert_eq!(status, "complete");
    assert_eq!(summary, "Reviewed day");
    assert!(lease.is_none());

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
         CREATE TABLE {namespace}.actors(
             id text PRIMARY KEY,active boolean NOT NULL,kind text NOT NULL,
             created_at timestamptz NOT NULL DEFAULT now()
         );
         CREATE TABLE {namespace}.actor_roles(actor_id text NOT NULL,role text NOT NULL);
         CREATE TABLE {namespace}.labor_calendar_settings(
             singleton boolean PRIMARY KEY,timezone text NOT NULL,starts_on date NOT NULL
         );
         CREATE TABLE {namespace}.installation_preferences(language text NOT NULL);
         CREATE TABLE {namespace}.daily_developer_analyses(
             actor_id text NOT NULL,date date NOT NULL,timezone text NOT NULL,
             status text NOT NULL DEFAULT 'pending',started_at timestamptz,
             lease_token text,evidence jsonb,report jsonb,generated_at timestamptz,
             PRIMARY KEY(actor_id,date)
         );
         CREATE TABLE {namespace}.task_work_logs(
             id text PRIMARY KEY,actor_id text NOT NULL,date date NOT NULL,title text,
             minutes integer NOT NULL,note text,task_id text,removed_at timestamptz
         );
         CREATE TABLE {namespace}.tasks(id text PRIMARY KEY,title text,project_id text);
         CREATE TABLE {namespace}.task_history(
             id text PRIMARY KEY,task_id text NOT NULL,actor_id text NOT NULL,
             operation text NOT NULL,created_at timestamptz NOT NULL,project_id text
         );
         CREATE TABLE {namespace}.git_commit_authorship(
             project_id text,commit_sha text,repository_id text,actor_id text,
             occurred_at timestamptz,commit_message text
         );
         CREATE TABLE {namespace}.project_repositories(
             id text PRIMARY KEY,connection_id text,authorization_status text,archived boolean
         );
         CREATE TABLE {namespace}.git_connections(id text PRIMARY KEY,owner_actor_id text,status text);
         CREATE TABLE {namespace}.project_document_versions(
             id text,created_by text,created_at timestamptz,title text,content jsonb,project_id text
         );
         CREATE TABLE {namespace}.organization_document_versions(
             document_id text,revision bigint,created_by text,created_at timestamptz,title text,body text
         );
         CREATE TABLE {namespace}.audit_events(
             id text,actor_id text,operation text,created_at timestamptz,metadata jsonb,resource_id text
         );
         CREATE FUNCTION {namespace}.member_daily_availability(text,date) RETURNS jsonb
             LANGUAGE sql AS 'SELECT ''{{}}''::jsonb';
         INSERT INTO {namespace}.actors(id,active,kind,created_at)
             VALUES('member-a',true,'human',(current_date-1)::timestamp);
         INSERT INTO {namespace}.actor_roles VALUES('member-a','developer');
         INSERT INTO {namespace}.labor_calendar_settings VALUES(true,'UTC',current_date-1);
         INSERT INTO {namespace}.installation_preferences VALUES('en');
         INSERT INTO {namespace}.task_work_logs(id,actor_id,date,title,minutes,note)
             VALUES('log-a','member-a',current_date-1,'Reviewed changes',45,'reviewed a pull request');"
    ))
}
