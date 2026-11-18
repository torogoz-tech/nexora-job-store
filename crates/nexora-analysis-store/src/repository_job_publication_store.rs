use crate::{
    AnalysisStore, RepositoryJobLease, RepositoryRuntimeScope, SecretGenerator,
    audit_writer::AuditWriter, repository_job_execution_authority::RepositoryJobExecutionAuthority,
    repository_job_lease_store::RepositoryJobLeaseStore,
};
use nexora_platform::{PlatformError, RepositoryBinding, RepositoryJob, RepositoryJobState};
use nexora_runtime::QuerySnapshot;
use postgres::Transaction;
use serde_json::{Value, json};

pub(crate) struct RepositoryJobPublicationStore<'a> {
    store: &'a mut AnalysisStore,
}
impl<'a> RepositoryJobPublicationStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }
    fn scope(
        tx: &mut Transaction<'_>,
        job: &RepositoryJob,
    ) -> Result<RepositoryRuntimeScope, PlatformError> {
        let tenant = tx
            .query_opt("SELECT id FROM installation WHERE singleton", &[])
            .map_err(AnalysisStore::database_error)?
            .ok_or(PlatformError::Storage)?
            .get(0);
        Ok(RepositoryRuntimeScope {
            tenant_id: tenant,
            repository_id: job.repository_id.clone(),
            pinned_commit: job
                .pinned_commit
                .clone()
                .ok_or(PlatformError::Conflict("job commit has not been pinned"))?,
        })
    }
    fn binding(
        tx: &mut Transaction<'_>,
        job: &RepositoryJob,
        lease: &RepositoryJobLease,
    ) -> Result<RepositoryBinding, PlatformError> {
        let digest = SecretGenerator::digest(&lease.token);
        let row = tx.query_opt("SELECT project_id,repository_id,runtime_tenant_id,runtime_repository_id,snapshot_id,payload_hash,pinned_commit,source_generation,job_id,attempt,revision,published_at::text FROM repository_bindings WHERE repository_id=$1 AND job_id=$2 AND attempt=$3 AND completion_digest=$4 FOR SHARE", &[&job.repository_id,&job.id,&lease.fence,&digest]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("published binding has been replaced"))?;
        Ok(RepositoryBinding {
            project_id: row.get(0),
            repository_id: row.get(1),
            runtime_tenant_id: row.get(2),
            runtime_repository_id: row.get(3),
            snapshot_id: row.get(4),
            payload_hash: row.get(5),
            pinned_commit: row.get(6),
            source_generation: row.get(7),
            job_id: row.get(8),
            attempt: row.get(9),
            revision: row.get(10),
            published_at: row.get(11),
        })
    }
    pub fn pin(
        &mut self,
        lease: &RepositoryJobLease,
        commit: &str,
    ) -> Result<RepositoryRuntimeScope, PlatformError> {
        if !matches!(commit.len(), 40 | 64)
            || !commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(PlatformError::Validation(
                "invalid commit object identifier",
            ));
        }
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row = RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
        let value: Value = row.get(0);
        let mut job: RepositoryJob = AnalysisStore::decode(value.clone())?;
        if job
            .pinned_commit
            .as_deref()
            .is_some_and(|pinned| pinned != commit)
        {
            return Err(PlatformError::Conflict("job commit is immutable"));
        }
        if job.pinned_commit.is_none() {
            tx.execute("UPDATE repository_jobs SET pinned_commit=$2,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1", &[&job.id,&commit]).map_err(AnalysisStore::database_error)?;
            AuditWriter::append(
                &mut tx,
                Some(&job.requested_by),
                value["credential_id"].as_str(),
                "repository.job_pinned",
                Some(&job.id),
                json!({"commit":commit,"attempt":lease.fence}),
            )?;
            job.pinned_commit = Some(commit.into());
        }
        // Audit/storage work may take time; do not commit an attempt that expired meanwhile.
        RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
        let scope = Self::scope(&mut tx, &job)?;
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(scope)
    }
    pub fn complete(
        &mut self,
        lease: &RepositoryJobLease,
        snapshot: &QuerySnapshot,
    ) -> Result<RepositoryBinding, PlatformError> {
        self.complete_with_selection(lease, snapshot, None)
    }
    pub fn complete_with_selection(
        &mut self,
        lease: &RepositoryJobLease,
        snapshot: &QuerySnapshot,
        selection: Option<&Value>,
    ) -> Result<RepositoryBinding, PlatformError> {
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row = RepositoryJobExecutionAuthority::lock(&mut tx, lease, true)?;
        let value: Value = row.get(0);
        let job: RepositoryJob = AnalysisStore::decode(value.clone())?;
        let scope = Self::scope(&mut tx, &job)?;
        if !value["analysis_profile_plan"].is_null() && selection.is_none() {
            return Err(PlatformError::Validation(
                "multi-profile jobs require an analysis selection report",
            ));
        }
        if let Some(selection) = selection {
            crate::analysis_profile_report_store::AnalysisProfileReportStore::validate(
                &mut tx,
                &value["analysis_profile_plan"],
                selection,
            )?;
            if !value["analysis_profile_selection"].is_null()
                && value["analysis_profile_selection"] != *selection
            {
                return Err(PlatformError::Conflict(
                    "completed analysis profile report is immutable",
                ));
            }
        }
        if snapshot.tenant_id.0 != scope.tenant_id
            || snapshot.repository_id.0 != scope.repository_id
        {
            return Err(PlatformError::Conflict(
                "snapshot is outside the registered runtime scope",
            ));
        }
        if job.state == RepositoryJobState::Succeeded {
            tx.execute(
                "UPDATE code_snapshot_embeddings SET published=true WHERE project_id=$1 AND repository_id=$2 AND snapshot_id=$3",
                &[&job.project_id, &job.repository_id, &snapshot.id.0],
            )
            .map_err(AnalysisStore::database_error)?;
            let binding = Self::binding(&mut tx, &job, lease)?;
            if binding.snapshot_id != snapshot.id.0 || binding.payload_hash != snapshot.payload_hash
            {
                return Err(PlatformError::Conflict("completed job result is immutable"));
            }
            tx.commit().map_err(AnalysisStore::database_error)?;
            return Ok(binding);
        }
        let projection = tx.query_opt("SELECT q.summary,q.summary_hash FROM nexora_runtime.snapshots s JOIN nexora_runtime.query_snapshots q ON q.tenant_id=s.tenant_id AND q.repository_id=s.repository_id AND q.snapshot_id=s.snapshot_id AND q.payload_hash=s.payload_hash WHERE s.tenant_id=$1 AND s.repository_id=$2 AND s.snapshot_id=$3 AND s.payload_hash=$4 AND s.state='ready' AND q.version=$5 AND octet_length(q.summary)<=1048576 FOR SHARE OF s,q", &[&scope.tenant_id,&scope.repository_id,&snapshot.id.0,&snapshot.payload_hash,&QuerySnapshot::VERSION]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("snapshot projection is not ready or compatible"))?;
        let summary: Vec<u8> = projection.get(0);
        if SecretGenerator::checksum(&summary) != projection.get::<_, String>(1)
            || serde_json::from_slice::<Value>(&summary).is_err()
        {
            return Err(PlatformError::Conflict(
                "snapshot projection checksum mismatch",
            ));
        }
        let digest = SecretGenerator::digest(&lease.token);
        let connection = value["connection_id"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let connection_revision = value["connection_revision"]
            .as_i64()
            .ok_or(PlatformError::Storage)?;
        tx.execute("INSERT INTO repository_bindings AS b(repository_id,project_id,runtime_tenant_id,runtime_repository_id,snapshot_id,payload_hash,pinned_commit,source_generation,repository_revision,connection_id,connection_revision,job_id,attempt,completion_digest) VALUES($1,$2,$3,$1,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT(repository_id) DO UPDATE SET project_id=EXCLUDED.project_id,runtime_tenant_id=EXCLUDED.runtime_tenant_id,runtime_repository_id=EXCLUDED.runtime_repository_id,snapshot_id=EXCLUDED.snapshot_id,payload_hash=EXCLUDED.payload_hash,pinned_commit=EXCLUDED.pinned_commit,source_generation=EXCLUDED.source_generation,repository_revision=EXCLUDED.repository_revision,connection_id=EXCLUDED.connection_id,connection_revision=EXCLUDED.connection_revision,job_id=EXCLUDED.job_id,attempt=EXCLUDED.attempt,completion_digest=EXCLUDED.completion_digest,revision=b.revision+1,published_at=clock_timestamp()", &[&job.repository_id,&job.project_id,&scope.tenant_id,&snapshot.id.0,&snapshot.payload_hash,&scope.pinned_commit,&job.source_generation,&job.repository_revision,&connection,&connection_revision,&job.id,&lease.fence,&digest]).map_err(AnalysisStore::database_error)?;
        AuditWriter::append(
            &mut tx,
            Some(&job.requested_by),
            value["credential_id"].as_str(),
            "repository.job_completed",
            Some(&job.id),
            json!({"project_id":job.project_id,"repository_id":job.repository_id,"snapshot_id":snapshot.id.0,"commit":scope.pinned_commit,"attempt":lease.fence}),
        )?;
        RepositoryJobLeaseStore::authorize(&mut tx, &row)?;
        let count = tx.execute("UPDATE repository_jobs SET state='succeeded',snapshot_id=$2,error_code=NULL,lease_owner=NULL,lease_token_digest=NULL,lease_until=NULL,revision=revision+1,updated_at=clock_timestamp(),finished_at=clock_timestamp() WHERE id=$1 AND state='running' AND attempt=$3 AND lease_token_digest=$4 AND lease_until>clock_timestamp()", &[&job.id,&snapshot.id.0,&lease.fence,&digest]).map_err(AnalysisStore::database_error)?;
        if count != 1 {
            return Err(PlatformError::Conflict(
                "job lease expired before completion",
            ));
        }
        tx.execute(
            "UPDATE code_snapshot_embeddings SET published=true WHERE project_id=$1 AND repository_id=$2 AND snapshot_id=$3",
            &[&job.project_id, &job.repository_id, &snapshot.id.0],
        )
        .map_err(AnalysisStore::database_error)?;
        if let Some(selection) = selection {
            tx.execute(
                "UPDATE repository_jobs SET analysis_profile_selection=$2 WHERE id=$1",
                &[&job.id, selection],
            )
            .map_err(AnalysisStore::database_error)?;
        }
        let binding = Self::binding(&mut tx, &job, lease)?;
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(binding)
    }
}
