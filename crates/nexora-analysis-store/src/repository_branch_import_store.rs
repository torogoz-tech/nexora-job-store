use crate::{
    AnalysisStore, GitNetworkPolicy, RepositoryJobLease,
    git_author_observation_writer::GitAuthorObservationWriter, git_author_policy::GitAuthorPolicy,
    git_commit_identity::GitCommitIdentity, github_client::GithubClient,
    github_historical_commit_writer::GithubHistoricalCommitWriter,
    github_repository_proof::GithubRepositoryProof,
    repository_job_execution_authority::RepositoryJobExecutionAuthority,
    repository_job_source_request::RepositoryJobSourceRequest,
};
use nexora_platform::{GitProvider, PlatformError};
use std::collections::HashSet;
impl AnalysisStore {
    pub fn import_repository_branches(
        &mut self,
        lease: &RepositoryJobLease,
        policy: &GitNetworkPolicy,
    ) -> Result<(), PlatformError> {
        let request = RepositoryJobSourceRequest::load(self, lease, policy)?;
        if request.connection.provider != GitProvider::Github {
            return Ok(());
        }
        let (_, token) = GithubRepositoryProof::check(
            self,
            &request.connection,
            &request.remote_name,
            Some(&request.remote_id),
            policy,
        )?;
        let client = GithubClient::new(policy);
        let mut branches = Vec::new();
        let mut names = HashSet::new();
        let mut page = 1;
        loop {
            let value = client.get(
                &format!(
                    "/repos/{}/branches?per_page=100&page={page}",
                    token.full_name
                ),
                &token.access_token,
            )?;
            let items = value
                .as_array()
                .filter(|items| items.len() <= 100)
                .ok_or(PlatformError::Unavailable)?;
            for item in items {
                let name = item["name"]
                    .as_str()
                    .filter(|name| {
                        !name.is_empty()
                            && name.len() <= 1024
                            && !name.chars().any(char::is_control)
                    })
                    .ok_or(PlatformError::Unavailable)?;
                let head = item["commit"]["sha"]
                    .as_str()
                    .filter(|sha| GitCommitIdentity::valid(sha))
                    .ok_or(PlatformError::Unavailable)?;
                if !names.insert(name.to_owned()) {
                    return Err(PlatformError::Unavailable);
                }
                branches.push((name.to_owned(), head.to_owned()));
            }
            if items.len() < 100 {
                break;
            }
            page += 1;
        }
        for (branch, head) in branches {
            let mut tx = self.client().transaction().map_err(Self::database_error)?;
            RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
            tx.execute("INSERT INTO repository_branch_imports(repository_id,branch_name,target_commit) VALUES($1,$2,$3) ON CONFLICT(repository_id,branch_name) DO UPDATE SET target_commit=EXCLUDED.target_commit,next_page=1,complete=false,updated_at=clock_timestamp() WHERE repository_branch_imports.target_commit<>EXCLUDED.target_commit",&[&request.job.repository_id,&branch,&head]).map_err(Self::database_error)?;
            tx.commit().map_err(Self::database_error)?;
            loop {
                let mut tx = self.client().transaction().map_err(Self::database_error)?;
                RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
                let cursor=tx.query_one("SELECT target_commit,next_page,complete FROM repository_branch_imports WHERE repository_id=$1 AND branch_name=$2 FOR UPDATE",&[&request.job.repository_id,&branch]).map_err(Self::database_error)?;
                if cursor.get::<_, String>(0) != head {
                    return Err(PlatformError::Conflict("branch import target changed"));
                }
                let page: i64 = cursor.get(1);
                let complete: bool = cursor.get(2);
                tx.commit().map_err(Self::database_error)?;
                if complete {
                    break;
                }
                let value = client.get(
                    &format!(
                        "/repos/{}/commits?sha={head}&per_page=100&page={page}",
                        token.full_name
                    ),
                    &token.access_token,
                )?;
                let commits = value
                    .as_array()
                    .filter(|items| items.len() <= 100)
                    .ok_or(PlatformError::Unavailable)?;
                let mut tx = self.client().transaction().map_err(Self::database_error)?;
                RepositoryJobExecutionAuthority::lock(&mut tx, lease, false)?;
                let cursor=tx.query_one("SELECT target_commit,next_page,complete FROM repository_branch_imports WHERE repository_id=$1 AND branch_name=$2 FOR UPDATE",&[&request.job.repository_id,&branch]).map_err(Self::database_error)?;
                if cursor.get::<_, String>(0) != head
                    || cursor.get::<_, i64>(1) != page
                    || cursor.get::<_, bool>(2)
                {
                    return Err(PlatformError::Conflict("branch import cursor changed"));
                }
                GitAuthorPolicy::lock(&mut tx)?;
                for commit in commits {
                    GithubHistoricalCommitWriter::record(
                        &mut tx,
                        &request,
                        commit,
                        &head,
                        Some(&branch),
                    )?;
                }
                GitAuthorObservationWriter::attribute(
                    &mut tx,
                    None,
                    Some(&request.job.repository_id),
                )?;
                tx.execute("UPDATE repository_branch_imports SET next_page=next_page+1,complete=$3,updated_at=clock_timestamp() WHERE repository_id=$1 AND branch_name=$2",&[&request.job.repository_id,&branch,&(commits.len()<100)]).map_err(Self::database_error)?;
                tx.commit().map_err(Self::database_error)?;
            }
        }
        Ok(())
    }
}
