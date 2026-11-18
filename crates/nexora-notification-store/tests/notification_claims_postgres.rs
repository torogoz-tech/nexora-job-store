use nexora_database_connections::DatabaseConnections;
use nexora_notification_store::NotificationStore;
use postgres::{Client, Config, NoTls};
use std::path::PathBuf;

#[test]
fn claims_authorized_test_email_and_skips_an_unauthorized_recipient() {
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
        "nexora-notification-store-{}-{}",
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
    let mut admin = postgres_config(&host.to_string_lossy(), port, &user, &password, &database)
        .connect(NoTls)
        .unwrap();
    let namespace = format!(
        "nexora_platform_it_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let result = exercise_notification_claims(&mut admin, &settings, &namespace);
    admin
        .batch_execute(&format!("DROP SCHEMA IF EXISTS {namespace} CASCADE"))
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    result.unwrap();
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

fn exercise_notification_claims(
    admin: &mut Client,
    settings: &DatabaseConnections,
    namespace: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    admin.batch_execute(&format!(
        "CREATE SCHEMA {namespace};
         CREATE TABLE {namespace}.actors(id text PRIMARY KEY, active boolean NOT NULL, kind text NOT NULL);
         CREATE TABLE {namespace}.actor_grants(actor_id text NOT NULL, capability text NOT NULL, project_id text);
         CREATE TABLE {namespace}.installation_preferences(singleton boolean PRIMARY KEY, language text NOT NULL);
         CREATE TABLE {namespace}.user_email_changes(expires_at timestamptz, used_at timestamptz, attempts integer, token_secret bytea);
         CREATE TABLE {namespace}.member_invitations(expires_at timestamptz, status text, token_secret bytea);
         CREATE TABLE {namespace}.password_reset_requests(expires_at timestamptz, used_at timestamptz, token_secret bytea);
         CREATE TABLE {namespace}.email_notifications(
             event_key text PRIMARY KEY, kind text NOT NULL, project_id text, resource_id text NOT NULL,
             actor_id text NOT NULL, status text NOT NULL, attempts integer NOT NULL,
             available_at timestamptz NOT NULL, recipient_override text, delivered_at timestamptz
         );"
    ))?;
    admin.execute(
        "INSERT INTO actors(id,active,kind) VALUES('authorized',true,'human'),('inactive',false,'human')",
        &[],
    )?;
    admin.execute(
        "INSERT INTO actor_grants(actor_id,capability,project_id) VALUES('authorized','installation:manage',NULL)",
        &[],
    )?;
    admin.execute(
        "INSERT INTO installation_preferences(singleton,language) VALUES(true,'es')",
        &[],
    )?;
    admin.execute(
        "INSERT INTO email_notifications(event_key,kind,resource_id,actor_id,status,attempts,available_at,recipient_override)
         VALUES('authorized-event','test','authorized-resource','authorized','pending',0,clock_timestamp(),'admin@example.test'),
               ('inactive-event','test','inactive-resource','inactive','pending',0,clock_timestamp(),'inactive@example.test')",
        &[],
    )?;

    let mut store = NotificationStore::connect(settings, namespace)?;
    let authorized = store.claim_email_notification(false)?.unwrap();
    assert_eq!(authorized.event_key, "authorized-event");
    assert_eq!(authorized.attempts, 1);
    assert_eq!(authorized.email, "admin@example.test");
    assert_eq!(authorized.language, "es");
    store.finish_email_notification(&authorized.event_key, authorized.attempts, "sent")?;

    assert!(store.claim_email_notification(false)?.is_none());
    drop(store);

    let rows = admin.query(
        "SELECT event_key,status,attempts FROM email_notifications ORDER BY event_key",
        &[],
    )?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<_, String>(1), "sent");
    assert_eq!(rows[0].get::<_, i32>(2), 1);
    assert_eq!(rows[1].get::<_, String>(1), "skipped");
    assert_eq!(rows[1].get::<_, i32>(2), 1);
    Ok(())
}
