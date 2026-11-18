use crate::{
    AnalysisStore, SecretGenerator, audit_writer::AuditWriter,
    repository_webhook_job_authority::RepositoryWebhookJobAuthority,
};
use nexora_platform::PlatformError;
use serde_json::json;
pub(crate) struct RepositoryWebhookAnalysisStore;
impl RepositoryWebhookAnalysisStore {
    /// Operator worker entrypoint. Pending requests survive restarts and active-job contention.
    pub fn dispatch(store: &mut AnalysisStore, limit: u16) -> Result<(), PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        for _ in 0..limit {
            let row=tx.query_opt("SELECT q.id FROM repository_analysis_requests q JOIN project_repositories r ON r.id=q.repository_id WHERE q.state='pending' AND (q.source_generation<>r.source_generation OR NOT EXISTS(SELECT 1 FROM repository_jobs j WHERE j.repository_id=q.repository_id AND j.state IN ('queued','running'))) ORDER BY q.created_at,q.id FOR UPDATE OF q SKIP LOCKED LIMIT 1",&[]).map_err(AnalysisStore::database_error)?;
            let Some(row) = row else { break };
            let event: String = row.get(0);
            let q = RepositoryWebhookJobAuthority::request(&mut tx, &event)?;
            if let Err(error) = RepositoryWebhookJobAuthority::consent(&mut tx, &q) {
                let state = match error {
                    PlatformError::Conflict(_) => "superseded",
                    PlatformError::Unauthorized
                    | PlatformError::Forbidden
                    | PlatformError::NotFound => "rejected",
                    other => return Err(other),
                };
                tx.execute("UPDATE repository_analysis_requests SET state=$2,updated_at=clock_timestamp() WHERE id=$1",&[&event,&state]).map_err(AnalysisStore::database_error)?;
                continue;
            }
            let text = |key: &str| q[key].as_str().ok_or(PlatformError::Storage);
            let project = text("project_id")?;
            let repo = text("repository_id")?;
            let owner = text("requested_by")?;
            let conn = text("connection_id")?;
            let commit = text("source_commit")?;
            let revision = q["repository_revision"]
                .as_i64()
                .ok_or(PlatformError::Storage)?;
            let generation = q["source_generation"]
                .as_i64()
                .ok_or(PlatformError::Storage)?;
            let conn_revision = q["connection_revision"]
                .as_i64()
                .ok_or(PlatformError::Storage)?;
            let id = SecretGenerator::id("job")?;
            let created=tx.query_opt("INSERT INTO repository_jobs(id,project_id,repository_id,requested_by,credential_id,credential_method,kind,repository_revision,source_generation,connection_id,connection_revision,pinned_commit,source_event_id) VALUES($1,$2,$3,$4,$5,'webhook','index',$6,$7,$8,$9,$10,$5) ON CONFLICT(repository_id) WHERE state IN ('queued','running') DO NOTHING RETURNING id",&[&id,&project,&repo,&owner,&event,&revision,&generation,&conn,&conn_revision,&commit]).map_err(AnalysisStore::database_error)?;
            if created.is_none() {
                break;
            }
            tx.execute("UPDATE repository_analysis_requests SET state='queued',job_id=$2,updated_at=clock_timestamp() WHERE id=$1",&[&event,&id]).map_err(AnalysisStore::database_error)?;
            AuditWriter::append(
                &mut tx,
                None,
                None,
                "repository.webhook_analysis_enqueued",
                Some(&id),
                json!({"source_event_id":event,"repository_id":repo,"project_id":project,"commit":commit,"source_generation":generation}),
            )?;
        }
        tx.commit().map_err(AnalysisStore::database_error)
    }
}
