use crate::{
    AnalysisStore, RepositoryJobLease, SecretGenerator, audit_writer::AuditWriter,
    platform_authority::PlatformAuthority, repository_job_source::RepositoryJobSource,
};
use nexora_platform::*;
use postgres::{Row, Transaction};
use serde_json::{Value, json};
use zeroize::Zeroizing;

pub(crate) struct RepositoryJobLeaseStore<'a> {
    store: &'a mut AnalysisStore,
}
impl<'a> RepositoryJobLeaseStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }
    fn duration(seconds: u16) -> Result<i64, PlatformError> {
        if !(1..=300).contains(&seconds) {
            return Err(PlatformError::Validation(
                "job lease must be between 1 and 300 seconds",
            ));
        }
        Ok(i64::from(seconds))
    }
    pub(crate) fn authorize(tx: &mut Transaction<'_>, row: &Row) -> Result<(), PlatformError> {
        let value: Value = row.get(0);
        if value["credential_method"] == "webhook" {
            return crate::repository_webhook_job_authority::RepositoryWebhookJobAuthority::authorize(tx,&value);
        }
        let job: RepositoryJob = AnalysisStore::decode(value.clone())?;
        let method = match value["credential_method"].as_str() {
            Some("session") => AuthMethod::Session,
            Some("api_key") => AuthMethod::ApiKey,
            _ => return Err(PlatformError::Storage),
        };
        let credential = value["credential_id"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        // The job recorded authenticated origin at enqueue. SQL authority is loaded afresh below.
        let ctx = AuthContext::new(
            job.requested_by.clone(),
            ActorKind::Agent,
            method,
            credential.into(),
            vec![],
            None,
        );
        PlatformAuthority::require(
            tx,
            &ctx,
            Capability::RepositoriesSync,
            Some(&job.project_id),
        )?;
        PlatformAuthority::project(tx, &job.project_id)?;
        let source = RepositoryJobSource::lock(
            tx,
            &ctx,
            &job.project_id,
            &job.repository_id,
            job.repository_revision,
        )?;
        if source.generation != job.source_generation
            || value["connection_id"].as_str() != Some(source.connection.as_str())
            || value["connection_revision"].as_i64() != Some(source.connection_revision)
        {
            return Err(PlatformError::Conflict("queued source authority changed"));
        }
        Ok(())
    }
    fn reject(tx: &mut Transaction<'_>, id: &str, code: &str) -> Result<(), PlatformError> {
        tx.execute("UPDATE repository_jobs SET state='failed',error_code=$2,revision=revision+1,lease_owner=NULL,lease_token_digest=NULL,lease_until=NULL,updated_at=clock_timestamp(),finished_at=clock_timestamp() WHERE id=$1", &[&id,&code]).map_err(AnalysisStore::database_error)?;
        AuditWriter::append(
            tx,
            None,
            None,
            "repository.job_rejected",
            Some(id),
            json!({"error_code":code}),
        )
    }
    fn rejection(error: &PlatformError) -> Option<&'static str> {
        match error {
            PlatformError::Unauthorized | PlatformError::Forbidden => Some("authority_revoked"),
            PlatformError::NotFound | PlatformError::Conflict(_) => Some("source_changed"),
            _ => None,
        }
    }
    pub fn claim(
        &mut self,
        worker: &str,
        seconds: u16,
    ) -> Result<Option<RepositoryJobLease>, PlatformError> {
        let seconds = Self::duration(seconds)?;
        if worker.is_empty()
            || worker.len() > 128
            || !worker
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(PlatformError::Validation("invalid worker identifier"));
        }
        crate::repository_webhook_analysis_store::RepositoryWebhookAnalysisStore::dispatch(
            self.store, 16,
        )?;
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        tx.execute("SET LOCAL nexora.worker_analysis_profiles='2'", &[])
            .map_err(AnalysisStore::database_error)?;
        // Bound rejected-job scanning; no worker credentials or authorization are cached in RAM.
        for _ in 0..16 {
            let row = tx.query_opt("SELECT to_jsonb(j) FROM repository_jobs j WHERE (state='queued' AND available_at<=clock_timestamp()) OR (state='running' AND lease_until<=clock_timestamp()) ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1", &[]).map_err(AnalysisStore::database_error)?;
            let Some(row) = row else {
                tx.commit().map_err(AnalysisStore::database_error)?;
                return Ok(None);
            };
            let job: RepositoryJob = AnalysisStore::decode(row.get(0))?;
            if job.attempt >= 3 {
                Self::reject(&mut tx, &job.id, "attempts_exhausted")?;
                continue;
            }
            if let Err(error) = Self::authorize(&mut tx, &row) {
                if let Some(code) = Self::rejection(&error) {
                    Self::reject(&mut tx, &job.id, code)?;
                    continue;
                }
                return Err(error);
            }
            let token = Zeroizing::new(SecretGenerator::token("nxl")?);
            let digest = SecretGenerator::digest(&token);
            let row = tx.query_one("UPDATE repository_jobs AS j SET state='running',attempt=attempt+1,revision=revision+1,lease_owner=$2,lease_token_digest=$3,lease_until=clock_timestamp()+$4::bigint*interval '1 second',updated_at=clock_timestamp() WHERE id=$1 RETURNING to_jsonb(j),lease_until::text", &[&job.id,&worker,&digest,&seconds]).map_err(AnalysisStore::database_error)?;
            let job: RepositoryJob = AnalysisStore::decode(row.get(0))?;
            let lease = RepositoryJobLease {
                fence: job.attempt,
                job,
                lease_until: row.get(1),
                token,
            };
            AuditWriter::append(
                &mut tx,
                None,
                None,
                "repository.job_claimed",
                Some(&lease.job.id),
                json!({"worker":worker,"attempt":lease.fence}),
            )?;
            tx.commit().map_err(AnalysisStore::database_error)?;
            return Ok(Some(lease));
        }
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(None)
    }
    pub fn renew(
        &mut self,
        lease: &RepositoryJobLease,
        seconds: u16,
    ) -> Result<RepositoryJobLease, PlatformError> {
        let seconds = Self::duration(seconds)?;
        let digest = SecretGenerator::digest(&lease.token);
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row = tx.query_opt("SELECT to_jsonb(j) FROM repository_jobs j WHERE id=$1 AND state='running' AND attempt=$2 AND lease_token_digest=$3 AND lease_until>clock_timestamp() FOR UPDATE", &[&lease.job.id,&lease.fence,&digest]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("job lease is no longer current"))?;
        if let Err(error) = Self::authorize(&mut tx, &row) {
            if let Some(code) = Self::rejection(&error) {
                Self::reject(&mut tx, &lease.job.id, code)?;
                tx.commit().map_err(AnalysisStore::database_error)?;
            }
            return Err(error);
        }
        let row = tx.query_opt("UPDATE repository_jobs AS j SET lease_until=clock_timestamp()+$2::bigint*interval '1 second',updated_at=clock_timestamp() WHERE id=$1 AND lease_until>clock_timestamp() RETURNING to_jsonb(j),lease_until::text", &[&lease.job.id,&seconds]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("job lease expired during renewal"))?;
        let job: RepositoryJob = AnalysisStore::decode(row.get(0))?;
        let result = RepositoryJobLease {
            fence: job.attempt,
            job,
            lease_until: row.get(1),
            token: lease.token.clone(),
        };
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(result)
    }
}
