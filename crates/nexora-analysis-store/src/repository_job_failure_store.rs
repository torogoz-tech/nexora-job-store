use crate::{AnalysisStore, RepositoryJobLease, SecretGenerator, audit_writer::AuditWriter};
use nexora_platform::{PlatformError, RepositoryJob, RepositoryJobFailure};
use serde_json::{Value, json};

pub(crate) struct RepositoryJobFailureStore;
impl RepositoryJobFailureStore {
    pub fn record(
        store: &mut AnalysisStore,
        lease: &RepositoryJobLease,
        failure: RepositoryJobFailure,
    ) -> Result<RepositoryJob, PlatformError> {
        let digest = SecretGenerator::digest(&lease.token);
        let mut tx = store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        // A revoked source may be the failure itself. This capability terminates only
        // the currently leased attempt; it grants no source access or publication.
        let origin:Value=tx.query_opt("SELECT to_jsonb(j) FROM repository_jobs j WHERE id=$1 AND state='running' AND attempt=$2 AND lease_token_digest=$3 AND lease_until>clock_timestamp() FOR UPDATE", &[&lease.job.id, &lease.fence, &digest])
            .map_err(AnalysisStore::database_error)?
            .ok_or(PlatformError::Conflict("job lease is no longer current"))?.get(0);
        let retry = origin["credential_method"] == "webhook"
            && lease.fence < 3
            && matches!(
                failure,
                RepositoryJobFailure::SourceUnavailable | RepositoryJobFailure::StorageUnavailable
            );
        let delay = i64::from(lease.fence) * 5;
        AuditWriter::append(
            &mut tx,
            None,
            None,
            if retry {
                "repository.job_retry_scheduled"
            } else {
                "repository.job_failed"
            },
            Some(&lease.job.id),
            json!({"attempt":lease.fence,"error_code":failure.code(),"retry":retry}),
        )?;
        // Check the clock after locking and auditing, so expiry cannot be hidden by
        // transaction-start timestamps or time spent waiting on another operation.
        let value: Value = tx.query_opt("UPDATE repository_jobs AS j SET state=CASE WHEN $5 THEN 'queued' ELSE 'failed' END,error_code=$4,lease_owner=NULL,lease_token_digest=NULL,lease_until=NULL,revision=revision+1,updated_at=clock_timestamp(),finished_at=CASE WHEN $5 THEN NULL ELSE clock_timestamp() END,available_at=clock_timestamp()+$6::bigint*interval '1 second' WHERE id=$1 AND state='running' AND attempt=$2 AND lease_token_digest=$3 AND lease_until>clock_timestamp() RETURNING to_jsonb(j)", &[&lease.job.id, &lease.fence, &digest, &failure.code(),&retry,&delay])
            .map_err(AnalysisStore::database_error)?
            .ok_or(PlatformError::Conflict("job lease expired during failure recording"))?
            .get(0);
        let job = AnalysisStore::decode(value)?;
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(job)
    }
}
