use crate::{
    DatabaseConnections, connection_secrets::ConnectionSecrets, repository_lease::RepositoryLease,
    snapshot_checksum::SnapshotChecksum,
};
use nexora_core::{NexoraError, RepositoryId, SnapshotId, TenantId};
use nexora_storage::Snapshot;
use std::time::Duration;
pub struct PostgresSnapshots {
    client: postgres::Client,
    config: postgres::Config,
}
impl PostgresSnapshots {
    pub(crate) fn connect(
        c: &DatabaseConnections,
        s: &ConnectionSecrets,
    ) -> Result<Self, NexoraError> {
        let mut config = postgres::Config::new();
        config
            .host(&c.host)
            .port(c.postgres_port)
            .user(&c.postgres_user)
            .password(&s.postgres)
            .dbname(&c.postgres_database)
            .connect_timeout(Duration::from_secs(5))
            .options("-c statement_timeout=15000 -c lock_timeout=10000");
        let client = config.connect(postgres::NoTls).map_err(|_| Self::error())?;
        Ok(Self { client, config })
    }
    fn error() -> NexoraError {
        NexoraError::Storage("PostgreSQL request failed".into())
    }
    pub(crate) fn client(&mut self) -> &mut postgres::Client {
        &mut self.client
    }
    pub fn ping(&mut self) -> Result<(), NexoraError> {
        self.client
            .simple_query("SELECT 1")
            .map_err(|_| Self::error())?;
        Ok(())
    }
    pub fn initialize(&mut self) -> Result<(), NexoraError> {
        let mut tx = self.client.transaction().map_err(|_| Self::error())?;
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended('nexora-runtime-schema-v1',0))",
            &[],
        )
        .map_err(|_| Self::error())?;
        tx.batch_execute(include_str!("../migrations/0001_runtime.sql"))
            .map_err(|_| Self::error())?;
        let versions = tx
            .query("SELECT version FROM nexora_runtime.schema_versions", &[])
            .map_err(|_| Self::error())?;
        if versions.len() > 1
            || versions
                .first()
                .is_some_and(|r| ![1, 2, 3, 4].contains(&r.get::<_, i32>(0)))
        {
            return Err(NexoraError::Storage(
                "unsupported runtime database schema".into(),
            ));
        }
        let version = versions.first().map(|r| r.get::<_, i32>(0)).unwrap_or(1);
        if version < 3 {
            tx.batch_execute(include_str!("../migrations/0002_lossless_payload.sql"))
                .map_err(|_| Self::error())?;
            tx.batch_execute(include_str!("../migrations/0003_selective_queries.sql"))
                .map_err(|_| Self::error())?;
        }
        if version < 4 {
            tx.batch_execute(include_str!("../migrations/0004_knowledge.sql"))
                .map_err(|_| Self::error())?;
        }
        tx.commit().map_err(|_| Self::error())
    }
    pub(crate) fn lease(
        &self,
        tenant: &TenantId,
        repo: &RepositoryId,
    ) -> Result<RepositoryLease, NexoraError> {
        let mut connection = self
            .config
            .connect(postgres::NoTls)
            .map_err(|_| Self::error())?;
        let key = serde_json::to_string(&(&tenant.0, &repo.0)).map_err(|_| Self::error())?;
        connection
            .query_one("SELECT pg_advisory_lock(hashtextextended($1,0))", &[&key])
            .map_err(|_| Self::error())?;
        Ok(RepositoryLease {
            _connection: connection,
        })
    }
    pub fn prepare(&mut self, snapshot: &Snapshot) -> Result<String, NexoraError> {
        nexora_graph::GraphIntegrityValidator::validate(&snapshot.graph)?;
        let bytes = crate::snapshot_payload::SnapshotPayload::encode(snapshot)?;
        let hash = SnapshotChecksum::calculate(snapshot)?;

        let mut tx = self.client.transaction().map_err(|_| Self::error())?;
        // Snapshot writes grow with repository size; ordinary read deadlines remain unchanged.
        tx.batch_execute("SET LOCAL statement_timeout = 0")
            .map_err(|_| Self::error())?;
        tx.execute("INSERT INTO nexora_runtime.repositories(tenant_id,repository_id) VALUES($1,$2) ON CONFLICT DO NOTHING",&[&snapshot.tenant_id.0,&snapshot.repository_id.0]).map_err(|_|Self::error())?;
        tx.execute("INSERT INTO nexora_runtime.snapshots(tenant_id,repository_id,snapshot_id,payload,payload_hash,state) VALUES($1,$2,$3,$4,$5,'pending') ON CONFLICT DO NOTHING",&[&snapshot.tenant_id.0,&snapshot.repository_id.0,&snapshot.id.0,&bytes,&hash]).map_err(|_|Self::error())?;
        let row=tx.query_one("SELECT payload_hash FROM nexora_runtime.snapshots WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&snapshot.tenant_id.0,&snapshot.repository_id.0,&snapshot.id.0]).map_err(|_|Self::error())?;
        if row.get::<_, String>(0) != hash {
            return Err(NexoraError::Storage("snapshot is immutable".into()));
        }
        tx.commit().map_err(|_| Self::error())?;
        Ok(hash)
    }
    pub fn ready(&mut self, s: &Snapshot, hash: &str, activate: bool) -> Result<(), NexoraError> {
        let mut tx = self.client.transaction().map_err(|_| Self::error())?;
        let count=tx.execute("UPDATE nexora_runtime.snapshots SET state='ready' WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND payload_hash=$4",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0,&hash]).map_err(|_|Self::error())?;
        if count != 1 {
            return Err(Self::error());
        }
        if activate {
            tx.execute("UPDATE nexora_runtime.repositories SET current_snapshot_id=$3 WHERE tenant_id=$1 AND repository_id=$2",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0]).map_err(|_|Self::error())?;
        }
        tx.commit().map_err(|_| Self::error())
    }
    pub fn get(
        &mut self,
        tenant: &TenantId,
        repo: &RepositoryId,
        id: Option<&SnapshotId>,
    ) -> Result<Option<Snapshot>, NexoraError> {
        let row=if let Some(id)=id {
            self.client.query_opt("SELECT payload,payload_hash FROM nexora_runtime.snapshots WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND state='ready'",&[&tenant.0,&repo.0,&id.0])
        }else{
            self.client.query_opt("SELECT s.payload,s.payload_hash FROM nexora_runtime.repositories r JOIN nexora_runtime.snapshots s ON s.tenant_id=r.tenant_id AND s.repository_id=r.repository_id AND s.snapshot_id=r.current_snapshot_id WHERE r.tenant_id=$1 AND r.repository_id=$2 AND s.state='ready'",&[&tenant.0,&repo.0])
        }.map_err(|_|Self::error())?;
        let Some(row) = row else { return Ok(None) };
        let s: Snapshot = if row.columns()[0].type_() == &postgres::types::Type::BYTEA {
            let bytes: Vec<u8> = row.try_get(0).map_err(|_| Self::error())?;
            crate::snapshot_payload::SnapshotPayload::decode(&bytes)?
        } else if row.columns()[0].type_() == &postgres::types::Type::JSONB {
            // Read pre-migration records as well; publishing upgrades under the migration lock.
            serde_json::from_value(row.try_get(0).map_err(|_| Self::error())?)
                .map_err(|_| Self::error())?
        } else {
            return Err(Self::error());
        };
        if s.tenant_id != *tenant || s.repository_id != *repo || id.is_some_and(|id| s.id != *id) {
            return Err(Self::error());
        }
        let hash = SnapshotChecksum::calculate(&s)?;
        if hash != row.get::<_, String>(1) {
            return Err(NexoraError::Storage(
                "stored snapshot checksum mismatch".into(),
            ));
        }
        nexora_graph::GraphIntegrityValidator::validate(&s.graph)?;
        Ok(Some(s))
    }
}
