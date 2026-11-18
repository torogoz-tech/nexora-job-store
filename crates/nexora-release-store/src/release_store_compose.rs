use crate::{
    ReleaseStore, release_authentication::ReleaseAuthentication,
    release_authority::ReleaseAuthority, release_composition::ReleaseComposition,
    release_composition_job::ReleaseCompositionJob,
};
use nexora_analysis_store::{GitNetworkPolicy, RepositoryTransportPlan};
use nexora_platform::{AuthContext, PlatformError};
use nexora_source::{
    FetchedGitSource, GitFetchPolicy, GitFetcher, GitReleaseComposer, GitTransferLimits,
};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

impl ReleaseStore {
    pub fn compose_one_release(
        &mut self,
        policy: &GitNetworkPolicy,
        binary: &Path,
        root: &Path,
    ) -> Result<bool, PlatformError> {
        let candidate = self
            .client
            .client()
            .query_opt(
                "SELECT id,release_id FROM task_release_composition_jobs WHERE status='queued' OR (status='running' AND updated_at<clock_timestamp()-interval '2 minutes') ORDER BY created_at,id LIMIT 1",
                &[],
            )
            .map_err(ReleaseStore::database_error)?;
        let Some(candidate) = candidate else {
            return Ok(false);
        };
        let job_id: String = candidate.get(0);
        let release_id: String = candidate.get(1);
        self.compose_candidate(policy, binary, root, &job_id, &release_id)
    }

    fn compose_candidate(
        &mut self,
        policy: &GitNetworkPolicy,
        binary: &Path,
        root: &Path,
        job_id: &str,
        release_id: &str,
    ) -> Result<bool, PlatformError> {
        let lock = format!("release-compose:{release_id}");
        let locked = self
            .client
            .client()
            .query_one(
                "SELECT pg_try_advisory_lock(hashtextextended($1,0))",
                &[&lock],
            )
            .map_err(ReleaseStore::database_error)?
            .get::<_, bool>(0);
        if !locked {
            return Ok(false);
        }

        let result = self.compose_locked_candidate(policy, binary, root, job_id, release_id);
        let unlock = self
            .client
            .client()
            .query_one(
                "SELECT pg_advisory_unlock(hashtextextended($1,0))",
                &[&lock],
            )
            .map_err(ReleaseStore::database_error)?
            .get::<_, bool>(0);
        if !unlock {
            return Err(PlatformError::Storage);
        }
        result
    }

    fn compose_locked_candidate(
        &mut self,
        policy: &GitNetworkPolicy,
        binary: &Path,
        root: &Path,
        job_id: &str,
        release_id: &str,
    ) -> Result<bool, PlatformError> {
        let row = self
            .client
            .client()
            .query_opt(
                "SELECT id,release_id,project_id,release_revision,actor_id,credential_id,auth_method,record FROM task_release_composition_jobs WHERE id=$1 AND release_id=$2 AND (status='queued' OR (status='running' AND updated_at<clock_timestamp()-interval '2 minutes'))",
                &[&job_id, &release_id],
            )
            .map_err(ReleaseStore::database_error)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let job = ReleaseCompositionJob::from_row(&row)?;
        let outcome = self.compose_job(policy, binary, root, &job);
        match outcome {
            Ok(()) => Ok(true),
            Err(error) => {
                if matches!(error, PlatformError::Storage) {
                    return Err(error);
                }
                let mut record = job.record.clone();
                record["error"] = json!(Self::error_message(&error));
                self.client
                    .client()
                    .execute(
                        "UPDATE task_release_composition_jobs SET status='failed',record=$2,updated_at=clock_timestamp() WHERE id=$1 AND status IN ('queued','running')",
                        &[&job.id, &record],
                    )
                    .map_err(ReleaseStore::database_error)?;
                Ok(true)
            }
        }
    }

    fn compose_job(
        &mut self,
        policy: &GitNetworkPolicy,
        binary: &Path,
        root: &Path,
        job: &ReleaseCompositionJob,
    ) -> Result<(), PlatformError> {
        let context = ReleaseAuthentication::load_context(
            &mut self.client,
            job.auth_method,
            &job.credential_id,
        )?;
        if context.actor_id() != job.actor_id {
            return Err(PlatformError::Unauthorized);
        }
        ReleaseAuthority::authorize_publication(&mut self.client, &context, &job.project_id)?;
        ReleaseAuthority::authorize_release_tasks(
            &mut self.client,
            &context,
            &job.project_id,
            &job.release_id,
        )?;
        ReleaseAuthority::validate_release_pins(
            &mut self.client,
            &job.project_id,
            &job.release_id,
        )?;

        let release = self.release_projection(&job.project_id, &job.release_id)?;
        if release["revision"].as_i64() != Some(job.release_revision)
            || release["status"] != "prepared"
        {
            return Err(PlatformError::Conflict(
                "release changed before composition worker execution",
            ));
        }
        let branch = release["branch_name"]
            .as_str()
            .ok_or(PlatformError::Storage)?
            .to_owned();
        self.client
            .client()
            .execute(
                "UPDATE task_release_composition_jobs SET status='running',updated_at=clock_timestamp() WHERE id=$1",
                &[&job.id],
            )
            .map_err(ReleaseStore::database_error)?;

        let limits = GitTransferLimits {
            timeout: Duration::from_secs(120),
            max_pack_bytes: 128 * 1024 * 1024,
            max_repository_bytes: 512 * 1024 * 1024,
            max_repository_entries: 100_000,
        };
        let fetcher = GitFetcher::new(binary.to_owned(), limits.clone())
            .map_err(|_| PlatformError::Unavailable)?;
        let composer = GitReleaseComposer::new(binary.to_owned(), limits)
            .map_err(|_| PlatformError::Unavailable)?;
        let repositories = self
            .client
            .client()
            .query(
                "SELECT repository_id FROM task_release_repositories WHERE release_id=$1 ORDER BY repository_id",
                &[&job.release_id],
            )
            .map_err(ReleaseStore::database_error)?;
        if repositories.is_empty() {
            return Err(PlatformError::Validation(
                "release composition requires at least one repository",
            ));
        }
        let mut record = job.record.clone();
        if !record["repositories"].is_object() {
            record["repositories"] = json!({});
        }

        for (index, repository_row) in repositories.iter().enumerate() {
            let repository_id: String = repository_row.get(0);
            self.compose_repository(
                policy,
                &fetcher,
                &composer,
                root,
                job,
                &context,
                &repository_id,
                &branch,
                index,
                &mut record,
            )?;
        }
        self.finish_job(job, &context, &record)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_repository(
        &mut self,
        policy: &GitNetworkPolicy,
        fetcher: &GitFetcher,
        composer: &GitReleaseComposer,
        root: &Path,
        job: &ReleaseCompositionJob,
        context: &AuthContext,
        repository_id: &str,
        branch: &str,
        index: usize,
        record: &mut Value,
    ) -> Result<(), PlatformError> {
        ReleaseAuthority::authorize_publication(&mut self.client, context, &job.project_id)?;
        let commits = self.release_commits(&job.release_id, repository_id)?;
        ReleaseComposition::validate_commit_count(commits.len())?;
        let (connection, repository, authority) = ReleaseAuthority::lock_repository_source(
            &mut self.client,
            context,
            &job.project_id,
            repository_id,
        )?;
        let plan = self.provider_store.prepare_repository_transport_plan(
            &connection,
            &repository,
            policy,
            "main",
        )?;
        self.compose_and_publish_repository(
            policy,
            fetcher,
            composer,
            root,
            job,
            context,
            repository_id,
            branch,
            index,
            &commits,
            &connection,
            &repository,
            &authority,
            &plan,
            record,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_and_publish_repository(
        &mut self,
        policy: &GitNetworkPolicy,
        fetcher: &GitFetcher,
        composer: &GitReleaseComposer,
        root: &Path,
        job: &ReleaseCompositionJob,
        context: &AuthContext,
        repository_id: &str,
        branch: &str,
        index: usize,
        commits: &[String],
        connection: &nexora_platform::GitConnection,
        repository: &nexora_platform::ProjectRepository,
        authority: &Value,
        plan: &RepositoryTransportPlan,
        record: &mut Value,
    ) -> Result<(), PlatformError> {
        if record["repositories"][repository_id]["base_sha"].is_string()
            && record["repositories"][repository_id]["base_sha"] != plan.base_commit()
        {
            return Err(PlatformError::Conflict(
                "main changed after release composition began; prepare a new release revision",
            ));
        }

        let directory = Self::working_directory(root, &job.id, index)?;
        if directory.exists() {
            fs::remove_dir_all(&directory).map_err(|_| PlatformError::Storage)?;
        }
        let fetch_policy = GitFetchPolicy::new(plan.origin())
            .map_err(|_| PlatformError::Validation("invalid provider Git transport origin"))?;
        let fetched = fetcher
            .fetch(
                &fetch_policy,
                plan.repository(),
                plan.base_commit(),
                plan.format(),
                plan.credentials(),
                &directory,
            )
            .map_err(|_| {
                PlatformError::Conflict("release source fetch failed; no branch was published")
            })?;

        let result = self.compose_fetched_repository(
            policy,
            fetcher,
            composer,
            &fetch_policy,
            &fetched,
            job,
            context,
            repository_id,
            branch,
            commits,
            connection,
            repository,
            authority,
            plan,
            record,
        );
        let cleanup = fs::remove_dir_all(&directory);
        if result.is_ok() && cleanup.is_err() {
            return Err(PlatformError::Storage);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_fetched_repository(
        &mut self,
        policy: &GitNetworkPolicy,
        fetcher: &GitFetcher,
        composer: &GitReleaseComposer,
        fetch_policy: &GitFetchPolicy,
        fetched: &FetchedGitSource,
        job: &ReleaseCompositionJob,
        context: &AuthContext,
        repository_id: &str,
        branch: &str,
        commits: &[String],
        connection: &nexora_platform::GitConnection,
        repository: &nexora_platform::ProjectRepository,
        authority: &Value,
        plan: &RepositoryTransportPlan,
        record: &mut Value,
    ) -> Result<(), PlatformError> {
        let _ = policy;
        for commit in commits {
            let format = if commit.len() == 64 {
                nexora_source::GitObjectFormat::Sha256
            } else {
                nexora_source::GitObjectFormat::Sha1
            };
            fetcher
                .fetch_into(
                    fetch_policy,
                    plan.repository(),
                    commit,
                    format,
                    plan.credentials(),
                    fetched.bare(),
                )
                .map_err(|_| {
                    PlatformError::Conflict(
                        "selected task commit cannot be fetched from its authorized provider",
                    )
                })?;
        }
        let message = format!(
            "Nexora release {}\n\nSelected commits:\n{}\n",
            job.release_id,
            commits.join("\n")
        );
        let head = composer
            .compose(
                fetched.bare(),
                plan.base_commit(),
                commits,
                &message,
            )
            .map_err(|_| {
                PlatformError::Conflict(
                    "selected commits conflict or depend on unselected work; resolve the task commits before composing this release",
                )
            })?;
        if record["repositories"][repository_id]["head_sha"].is_string()
            && record["repositories"][repository_id]["head_sha"] != head
        {
            return Err(PlatformError::Conflict(
                "release composition intent changed",
            ));
        }
        record["repositories"][repository_id] = json!({
            "base_sha": plan.base_commit(),
            "head_sha": head,
            "commits": commits,
            "branch_name": branch,
            "provenance": "server_composed",
            "status": "prepared_to_push"
        });
        self.client
            .client()
            .execute(
                "UPDATE task_release_composition_jobs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
                &[&job.id, record],
            )
            .map_err(ReleaseStore::database_error)?;

        let (_, _, current_authority) = ReleaseAuthority::lock_repository_source(
            &mut self.client,
            context,
            &job.project_id,
            repository_id,
        )?;
        if &current_authority != authority {
            return Err(PlatformError::Conflict(
                "repository authority changed before release push",
            ));
        }
        ReleaseAuthority::authorize_publication(&mut self.client, context, &job.project_id)?;
        ReleaseAuthority::validate_release_pins(
            &mut self.client,
            &job.project_id,
            &job.release_id,
        )?;
        if self.release_revision(&job.project_id, &job.release_id)? != job.release_revision {
            return Err(PlatformError::Conflict(
                "release changed before branch creation",
            ));
        }

        let existing = self
            .provider_store
            .authorized_branch_head(connection, repository, policy, branch)?;
        if ReleaseComposition::branch_needs_push(existing.as_deref(), &head)? {
            composer
                .push_new(
                    fetched.bare(),
                    fetch_policy,
                    plan.repository(),
                    plan.credentials(),
                    &head,
                    branch,
                )
                .map_err(|_| PlatformError::Unavailable)?;
        }
        if self
            .provider_store
            .authorized_branch_head(connection, repository, policy, branch)?
            .as_deref()
            != Some(head.as_str())
        {
            return Err(PlatformError::Unavailable);
        }
        let (_, _, current_authority) = ReleaseAuthority::lock_repository_source(
            &mut self.client,
            context,
            &job.project_id,
            repository_id,
        )?;
        if &current_authority != authority {
            return Err(PlatformError::Conflict(
                "repository authority changed after release branch creation",
            ));
        }

        record["repositories"][repository_id]["status"] = json!("confirmed");
        self.client
            .client()
            .execute(
                "UPDATE task_release_repositories SET head_sha=$3,composition_status='composed',composition_evidence=$4 WHERE release_id=$1 AND repository_id=$2",
                &[
                    &job.release_id,
                    &repository_id,
                    &head,
                    &record["repositories"][repository_id],
                ],
            )
            .map_err(ReleaseStore::database_error)?;
        self.client
            .client()
            .execute(
                "UPDATE task_release_composition_jobs SET record=$2,updated_at=clock_timestamp() WHERE id=$1",
                &[&job.id, record],
            )
            .map_err(ReleaseStore::database_error)?;
        Ok(())
    }

    fn release_projection(&mut self, project: &str, release: &str) -> Result<Value, PlatformError> {
        self.client
            .client()
            .query_opt(
                "SELECT to_jsonb(r)||jsonb_build_object('tasks',COALESCE((SELECT jsonb_agg(to_jsonb(i) ORDER BY task_id) FROM task_release_items i WHERE release_id=r.id),'[]'::jsonb),'commits',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY task_id,sequence,repository_id,commit_sha) FROM task_release_commit_pins c WHERE release_id=r.id),'[]'::jsonb),'repositories',COALESCE((SELECT jsonb_agg(to_jsonb(i) ORDER BY repository_id) FROM task_release_repositories i WHERE release_id=r.id),'[]'::jsonb)) FROM task_releases r WHERE r.id=$1 AND r.project_id=$2",
                &[&release, &project],
            )
            .map_err(ReleaseStore::database_error)?
            .map(|row| row.get(0))
            .ok_or(PlatformError::NotFound)
    }

    fn release_revision(&mut self, project: &str, release: &str) -> Result<i64, PlatformError> {
        self.client
            .client()
            .query_opt(
                "SELECT revision FROM task_releases WHERE id=$1 AND project_id=$2 AND status='prepared'",
                &[&release, &project],
            )
            .map_err(ReleaseStore::database_error)?
            .map(|row| row.get(0))
            .ok_or(PlatformError::NotFound)
    }

    fn release_commits(
        &mut self,
        release: &str,
        repository: &str,
    ) -> Result<Vec<String>, PlatformError> {
        let rows = self
            .client
            .client()
            .query(
                "SELECT DISTINCT commit_sha,min(sequence) AS position FROM task_release_commit_pins WHERE release_id=$1 AND repository_id=$2 GROUP BY commit_sha ORDER BY position,commit_sha",
                &[&release, &repository],
            )
            .map_err(ReleaseStore::database_error)?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    fn finish_job(
        &mut self,
        job: &ReleaseCompositionJob,
        context: &AuthContext,
        record: &Value,
    ) -> Result<(), PlatformError> {
        let mut transaction = self
            .client
            .client()
            .transaction()
            .map_err(ReleaseStore::database_error)?;
        let revision: i64 = transaction
            .query_opt(
                "SELECT revision FROM task_releases WHERE id=$1 AND project_id=$2 AND status='prepared' FOR UPDATE",
                &[&job.release_id, &job.project_id],
            )
            .map_err(ReleaseStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        if revision != job.release_revision {
            return Err(PlatformError::Conflict(
                "release changed during composition",
            ));
        }
        transaction
            .execute(
                "UPDATE task_releases SET revision=revision+1 WHERE id=$1",
                &[&job.release_id],
            )
            .map_err(ReleaseStore::database_error)?;
        transaction
            .execute(
                "UPDATE task_release_composition_jobs SET status='succeeded',record=$2,updated_at=clock_timestamp() WHERE id=$1",
                &[&job.id, record],
            )
            .map_err(ReleaseStore::database_error)?;
        let audit_id = Self::random_id("evt")?;
        transaction
            .execute(
                "INSERT INTO audit_events(id,actor_id,credential_id,operation,resource_id,metadata) VALUES($1,$2,$3,'release.composed',$4,$5)",
                &[
                    &audit_id,
                    &context.actor_id(),
                    &context.credential_id(),
                    &job.release_id,
                    &json!({"job_id":job.id,"project_id":job.project_id,"evidence":record}),
                ],
            )
            .map_err(ReleaseStore::database_error)?;
        transaction.commit().map_err(ReleaseStore::database_error)
    }

    fn random_id(prefix: &str) -> Result<String, PlatformError> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| PlatformError::Unavailable)?;
        let mut id = String::with_capacity(prefix.len() + 33);
        id.push_str(prefix);
        id.push('_');
        for byte in bytes {
            use std::fmt::Write;
            write!(id, "{byte:02x}").map_err(|_| PlatformError::Unavailable)?;
        }
        Ok(id)
    }

    fn working_directory(
        root: &Path,
        job: &str,
        index: usize,
    ) -> Result<std::path::PathBuf, PlatformError> {
        if job.is_empty()
            || !job
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(PlatformError::Storage);
        }
        Ok(root.join(format!("{job}-repository-{index}")))
    }

    fn error_message(error: &PlatformError) -> &'static str {
        match error {
            PlatformError::Conflict(message) | PlatformError::Validation(message) => message,
            _ => "release composition failed; inspect authority, provider and worker availability",
        }
    }
}
