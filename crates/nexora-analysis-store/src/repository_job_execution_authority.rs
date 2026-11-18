use crate::{
    AnalysisStore, RepositoryJobLease, SecretGenerator,
    repository_job_lease_store::RepositoryJobLeaseStore,
};
use nexora_platform::PlatformError;
use postgres::{Row, Transaction};

pub(crate) struct RepositoryJobExecutionAuthority;
impl RepositoryJobExecutionAuthority {
    pub fn lock(
        tx: &mut Transaction<'_>,
        lease: &RepositoryJobLease,
        completed: bool,
    ) -> Result<Row, PlatformError> {
        let digest = SecretGenerator::digest(&lease.token);
        let row = tx.query_opt("SELECT to_jsonb(j) FROM repository_jobs j WHERE id=$1 AND attempt=$2 AND ((state='running' AND lease_token_digest=$3 AND lease_until>clock_timestamp()) OR ($4 AND state='succeeded' AND EXISTS(SELECT 1 FROM repository_bindings b WHERE b.job_id=j.id AND b.attempt=j.attempt AND b.completion_digest=$3))) FOR UPDATE OF j", &[&lease.job.id,&lease.fence,&digest,&completed]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("job lease is no longer current"))?;
        RepositoryJobLeaseStore::authorize(tx, &row)?;
        Ok(row)
    }
}
