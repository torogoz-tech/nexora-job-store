use crate::{
    AnalysisStore, GitNetworkPolicy, RepositoryJobLease,
    git_author_observation_writer::GitAuthorObservationWriter, git_author_policy::GitAuthorPolicy,
    git_commit_identity::GitCommitIdentity, github_client::GithubClient,
    github_repository_proof::GithubRepositoryProof,
    repository_job_execution_authority::RepositoryJobExecutionAuthority,
    repository_job_source_request::RepositoryJobSourceRequest,
};
use nexora_platform::{GitProvider, PlatformError};
impl AnalysisStore {
    pub fn import_repository_commits(
        &mut self,
        lease: &RepositoryJobLease,
        policy: &GitNetworkPolicy,
    ) -> Result<(), PlatformError> {
        let request = RepositoryJobSourceRequest::load(self, lease, policy)?;
        if request.connection.provider != GitProvider::Github {
            return Ok(());
        }
        // Hook health is recorded independently: a transient hook API failure must not
        // discard a valid source analysis or prevent recovery of historical commits.
        let _ = self.reconcile_github_webhook(policy);
        let target = request
            .job
            .pinned_commit
            .as_deref()
            .filter(|sha| GitCommitIdentity::valid(sha))
            .ok_or(PlatformError::Conflict("repository source is not pinned"))?;
        {
            let mut tx = self.client().transaction().map_err(Self::database_error)?;
            RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
            tx.execute("INSERT INTO repository_commit_imports(repository_id,target_commit,mode) VALUES($1,$2,'full') ON CONFLICT(repository_id) DO UPDATE SET baseline_commit=CASE WHEN repository_commit_imports.complete THEN repository_commit_imports.target_commit ELSE NULL END,mode=CASE WHEN repository_commit_imports.complete THEN 'compare' ELSE 'full' END,target_commit=EXCLUDED.target_commit,next_page=1,complete=false,imported_count=0,updated_at=clock_timestamp() WHERE repository_commit_imports.target_commit<>EXCLUDED.target_commit",&[&request.job.repository_id,&target]).map_err(Self::database_error)?;
            tx.execute("UPDATE repository_commit_imports SET complete=false,next_page=1,mode='full',baseline_commit=NULL,imported_count=0 WHERE repository_id=$1 AND complete AND EXISTS(SELECT 1 FROM git_commit_authorship WHERE repository_id=$1 AND commit_message IS NULL)",&[&request.job.repository_id]).map_err(Self::database_error)?;
            let complete: bool = tx
                .query_one(
                    "SELECT complete FROM repository_commit_imports WHERE repository_id=$1",
                    &[&request.job.repository_id],
                )
                .map_err(Self::database_error)?
                .get(0);
            tx.commit().map_err(Self::database_error)?;
            if complete {
                return Ok(());
            }
        }
        let (_, token) = GithubRepositoryProof::check(
            self,
            &request.connection,
            &request.remote_name,
            Some(&request.remote_id),
            policy,
        )?;
        let client = GithubClient::new(policy);
        loop {
            let mut tx = self.client().transaction().map_err(Self::database_error)?;
            RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
            let cursor=tx.query_one("SELECT target_commit,baseline_commit,mode,next_page,complete FROM repository_commit_imports WHERE repository_id=$1 FOR UPDATE",&[&request.job.repository_id]).map_err(Self::database_error)?;
            if cursor.get::<_, String>(0) != target {
                return Err(PlatformError::Conflict("commit import target changed"));
            }
            if cursor.get::<_, bool>(4) {
                tx.commit().map_err(Self::database_error)?;
                return Ok(());
            }
            let baseline: Option<String> = cursor.get(1);
            let mode: String = cursor.get(2);
            let page: i64 = cursor.get(3);
            tx.commit().map_err(Self::database_error)?;
            let path = if mode == "compare" {
                let base = baseline
                    .as_deref()
                    .filter(|sha| GitCommitIdentity::valid(sha))
                    .ok_or(PlatformError::Storage)?;
                format!(
                    "/repos/{}/compare/{base}...{target}?per_page=20&page={page}",
                    token.full_name
                )
            } else {
                format!(
                    "/repos/{}/commits?sha={target}&per_page=20&page={page}",
                    token.full_name
                )
            };
            let response = client.get(&path, &token.access_token);
            // A rewritten/deleted baseline cannot be used for incremental import.
            let fallback = mode == "compare"
                && match &response {
                    Ok(value) => !matches!(value["status"].as_str(), Some("ahead" | "identical")),
                    Err(PlatformError::NotFound) => true,
                    _ => false,
                };
            let mut tx = self.client().transaction().map_err(Self::database_error)?;
            RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
            let current=tx.query_one("SELECT target_commit,mode,next_page FROM repository_commit_imports WHERE repository_id=$1 FOR UPDATE",&[&request.job.repository_id]).map_err(Self::database_error)?;
            if current.get::<_, String>(0) != target
                || current.get::<_, String>(1) != mode
                || current.get::<_, i64>(2) != page
            {
                return Err(PlatformError::Conflict("commit import cursor changed"));
            }
            if fallback {
                tx.execute("UPDATE repository_commit_imports SET mode='full',baseline_commit=NULL,next_page=1,updated_at=clock_timestamp() WHERE repository_id=$1",&[&request.job.repository_id]).map_err(Self::database_error)?;
                tx.commit().map_err(Self::database_error)?;
                continue;
            }
            let value = response?;
            let commits = if mode == "compare" {
                value["commits"].as_array()
            } else {
                value.as_array()
            }
            .filter(|items| items.len() <= 20)
            .ok_or(PlatformError::Unavailable)?;
            GitAuthorPolicy::lock(&mut tx)?;
            for commit in commits {
                crate::github_historical_commit_writer::GithubHistoricalCommitWriter::record(
                    &mut tx, &request, commit, target, None,
                )?;
            }
            GitAuthorObservationWriter::attribute(&mut tx, None, Some(&request.job.repository_id))?;
            let complete = commits.len() < 20;
            tx.execute("UPDATE repository_commit_imports SET next_page=next_page+1,complete=$2,imported_count=imported_count+$3,updated_at=clock_timestamp() WHERE repository_id=$1",&[&request.job.repository_id,&complete,&(commits.len() as i64)]).map_err(Self::database_error)?;
            tx.commit().map_err(Self::database_error)?;
            if complete {
                return Ok(());
            }
        }
    }
}
