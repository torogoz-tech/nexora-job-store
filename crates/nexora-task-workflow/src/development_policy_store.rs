use crate::TaskWorkflowStore;
use nexora_platform::{DevelopmentPolicy, PlatformError};
use postgres::Transaction;

pub(crate) struct DevelopmentPolicyStore;

impl DevelopmentPolicyStore {
    pub(crate) fn lock(
        tx: &mut Transaction<'_>,
        project: &str,
        repository: &str,
    ) -> Result<DevelopmentPolicy, PlatformError> {
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            &[&format!("development-policy:{project}:{repository}")],
        )
        .map_err(TaskWorkflowStore::database_error)?;
        let row = tx
            .query_opt(
                "SELECT base_branch,branch_prefix,revision FROM repository_development_policies WHERE project_id=$1 AND repository_id=$2 FOR SHARE",
                &[&project, &repository],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(row
            .map(|row| DevelopmentPolicy {
                repository_id: repository.into(),
                base_branch: row.get(0),
                branch_prefix: row.get(1),
                revision: row.get(2),
            })
            .unwrap_or(DevelopmentPolicy {
                repository_id: repository.into(),
                base_branch: "develop".into(),
                branch_prefix: "feature".into(),
                revision: 0,
            }))
    }
}
