use crate::{
    AnalysisStore, SecretGenerator, git_author_identity::GitAuthorIdentity,
    git_commit_identity::GitCommitIdentity,
    repository_job_source_request::RepositoryJobSourceRequest,
};
use nexora_platform::PlatformError;
use postgres::Transaction;
use serde_json::Value;
pub(crate) struct GithubHistoricalCommitWriter;
impl GithubHistoricalCommitWriter {
    pub fn record(
        tx: &mut Transaction<'_>,
        request: &RepositoryJobSourceRequest,
        commit: &Value,
        target: &str,
        branch: Option<&str>,
    ) -> Result<(), PlatformError> {
        let sha = commit["sha"]
            .as_str()
            .filter(|sha| GitCommitIdentity::valid(sha))
            .ok_or(PlatformError::Unavailable)?;
        let author = &commit["commit"]["author"];
        let raw = author["email"].as_str().map(|email| {
            email
                .chars()
                .filter(|c| *c != '\0')
                .take(320)
                .collect::<String>()
        });
        let email = raw.as_deref().and_then(GitAuthorIdentity::email);
        let name = author["name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| *c != '\0')
            .take(256)
            .collect::<String>();
        let timestamp = commit["commit"]["committer"]["date"]
            .as_str()
            .or_else(|| author["date"].as_str())
            .ok_or(PlatformError::Unavailable)?;
        let timestamp =
            time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
                .map_err(|_| PlatformError::Unavailable)?
                .unix_timestamp() as f64;
        let message = commit["commit"]["message"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| *c != '\0')
            .take(16000)
            .collect::<String>();
        let id = SecretGenerator::id("gco")?;
        let scope = format!("history:{}", request.job.repository_id);
        tx.execute("INSERT INTO git_commit_authorship(id,provider,host,remote_repository_id,repository_id,project_id,commit_sha,author_name,author_email,author_email_raw,scope_id,delivery_id,occurred_at,commit_message) VALUES($1,'github',$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,to_timestamp($12),$13) ON CONFLICT(provider,host,remote_repository_id,commit_sha,project_id) DO UPDATE SET commit_message=EXCLUDED.commit_message",&[&id,&request.connection.host,&request.remote_id,&request.job.repository_id,&request.job.project_id,&sha,&name,&email,&raw,&scope,&target,&timestamp,&message]).map_err(AnalysisStore::database_error)?;
        if let Some(branch) = branch {
            tx.execute("INSERT INTO git_commit_branches(repository_id,commit_sha,branch_name) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",&[&request.job.repository_id,&sha,&branch]).map_err(AnalysisStore::database_error)?;
        }
        Ok(())
    }
}
