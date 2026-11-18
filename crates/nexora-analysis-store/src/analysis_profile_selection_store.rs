use crate::{
    AnalysisStore, RepositoryJobLease,
    repository_job_execution_authority::RepositoryJobExecutionAuthority,
};
use nexora_language::AnalysisProfilePackage;
use nexora_platform::PlatformError;
use serde_json::{Value, json};

pub(crate) struct AnalysisProfileSelectionStore;
impl AnalysisProfileSelectionStore {
    pub fn load(
        store: &mut AnalysisStore,
        lease: &RepositoryJobLease,
    ) -> Result<Option<Value>, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row = RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
        let value: Value = row.get(0);
        let plan = &value["analysis_profile_plan"];
        if plan.is_null() {
            tx.commit().map_err(AnalysisStore::database_error)?;
            return Ok(None);
        }
        let ids = plan["profile_version_ids"]
            .as_array()
            .ok_or(PlatformError::Storage)?;
        if ids.len() > 1024 {
            return Err(PlatformError::Storage);
        }
        let mut candidates = Vec::new();
        let mut bytes = 0usize;
        for id in ids {
            let id = id.as_str().ok_or(PlatformError::Storage)?;
            let row=tx.query_opt("SELECT checksum,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'),package FROM analysis_profile_versions WHERE id=$1", &[&id]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Storage)?;
            let package: AnalysisProfilePackage = AnalysisStore::decode(row.get(2))?;
            let checksum: String = row.get(0);
            if package.checksum().map_err(|_| PlatformError::Storage)? != checksum {
                return Err(PlatformError::Storage);
            }
            let package = serde_json::to_value(package).map_err(|_| PlatformError::Storage)?;
            bytes = bytes
                .checked_add(
                    serde_json::to_vec(&package)
                        .map_err(|_| PlatformError::Storage)?
                        .len(),
                )
                .ok_or(PlatformError::Storage)?;
            if bytes > 67_108_864 {
                return Err(PlatformError::Storage);
            }
            candidates.push(json!({"id":id,"checksum":checksum,"created_at":row.get::<_,String>(1),"package":package}));
        }
        tx.commit().map_err(AnalysisStore::database_error)?;
        Ok(Some(
            json!({"mode":plan["mode"],"policy":plan["policy"],"candidates":candidates}),
        ))
    }
}
