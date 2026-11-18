use crate::AnalysisStore;
use nexora_platform::{PlatformError, RepositoryJob};
use postgres::Transaction;
use serde_json::Value;
/// Source consent grants engine work; no developer session/API key is synthesized.
pub(crate) struct RepositoryWebhookJobAuthority;
impl RepositoryWebhookJobAuthority {
    pub fn request(tx: &mut Transaction<'_>, id: &str) -> Result<Value, PlatformError> {
        tx.query_opt("SELECT to_jsonb(q)||jsonb_build_object('provider',e.provider,'scope_id',e.scope_id,'source_commit',e.source_commit,'source_ref',e.source_ref) FROM repository_analysis_requests q JOIN repository_source_events e ON e.id=q.id AND e.repository_id=q.repository_id AND e.source_generation=q.source_generation WHERE q.id=$1 FOR SHARE OF q,e",&[&id]).map_err(AnalysisStore::database_error)?.map(|r|r.get(0)).ok_or(PlatformError::NotFound)
    }
    pub fn consent(tx: &mut Transaction<'_>, q: &Value) -> Result<(), PlatformError> {
        let text = |key: &str| q[key].as_str().ok_or(PlatformError::Storage);
        let project = text("project_id")?;
        let repo = text("repository_id")?;
        let conn = text("connection_id")?;
        tx.query_opt(
            "SELECT id FROM projects WHERE id=$1 AND NOT archived FOR SHARE",
            &[&project],
        )
        .map_err(AnalysisStore::database_error)?
        .ok_or(PlatformError::NotFound)?;
        // Match GitHub configuration rotation: settings before connection rows.
        if text("provider")? == "github" {
            let scope = text("scope_id")?;
            tx.query_opt("SELECT app_id FROM github_settings WHERE singleton AND enabled AND app_id=$1 FOR SHARE",&[&scope]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
        }
        let c=tx.query_opt("SELECT owner_actor_id,revision,provider,host FROM git_connections WHERE id=$1 AND status='active' FOR SHARE",&[&conn]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
        let owner: String = c.get(0);
        let provider: String = c.get(2);
        let host: String = c.get(3);
        if owner != text("requested_by")?
            || Some(c.get::<_, i64>(1)) != q["connection_revision"].as_i64()
            || provider != text("provider")?
        {
            return Err(PlatformError::Conflict(
                "webhook connection authority changed",
            ));
        }
        tx.query_opt(
            "SELECT id FROM actors WHERE id=$1 AND active FOR SHARE",
            &[&owner],
        )
        .map_err(AnalysisStore::database_error)?
        .ok_or(PlatformError::Unauthorized)?;
        let r=tx.query_opt("SELECT revision,source_generation,latest_source_commit,host,provider FROM project_repositories WHERE id=$1 AND project_id=$2 AND connection_id=$3 AND source_kind='git' AND authorization_status='authorized' AND NOT archived FOR SHARE",&[&repo,&project,&conn]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
        if Some(r.get::<_, i64>(0)) != q["repository_revision"].as_i64()
            || Some(r.get::<_, i64>(1)) != q["source_generation"].as_i64()
            || r.get::<_, Option<String>>(2).as_deref() != Some(text("source_commit")?)
            || r.get::<_, String>(3) != host
            || r.get::<_, String>(4) != provider
        {
            return Err(PlatformError::Conflict("webhook source changed"));
        }
        let scope = text("scope_id")?;
        match provider.as_str() {
            "github" => {}
            "gitlab" => {
                tx.query_opt("SELECT id FROM gitlab_webhooks WHERE id=$1 AND repository_id=$2 AND host=$3 AND active FOR SHARE",&[&scope,&repo,&host]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
            }
            _ => return Err(PlatformError::Storage),
        }
        Ok(())
    }
    pub fn authorize(tx: &mut Transaction<'_>, value: &Value) -> Result<(), PlatformError> {
        let job: RepositoryJob = AnalysisStore::decode(value.clone())?;
        let event = job
            .source_event_id
            .as_deref()
            .ok_or(PlatformError::Storage)?;
        let q = Self::request(tx, event)?;
        if value["credential_id"] != event
            || q["job_id"] != job.id
            || q["state"] != "queued"
            || q["repository_id"] != job.repository_id
            || q["project_id"] != job.project_id
            || q["requested_by"] != job.requested_by
            || q["repository_revision"] != job.repository_revision
            || q["source_generation"] != job.source_generation
            || q["connection_id"] != value["connection_id"]
            || q["connection_revision"] != value["connection_revision"]
            || q["source_commit"].as_str() != job.pinned_commit.as_deref()
        {
            return Err(PlatformError::Conflict(
                "webhook job origin does not match event",
            ));
        }
        Self::consent(tx, &q)
    }
}
