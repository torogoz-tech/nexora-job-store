use crate::{
    GitNetworkPolicy, SecretGenerator, TaskWorkflowStore, pull_request_client::PullRequestClient,
    task_contributor_code::TaskContributorCode, task_pre_pr_review::TaskPrePrReview,
    task_transition_provider::TaskTransitionProvider,
    task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
/// Explicit independent personal assessment. Public outcomes deliberately contain only counts.
pub(crate) struct TaskContributorRating;
impl TaskContributorRating {
    /// Captures bounded contributor evidence and persists knowledge-worker jobs.
    /// It never performs model inference in the release-worker process.
    pub fn enqueue(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        branch: &TaskTransitionBranch,
        _base: Option<&str>,
        policy: &GitNetworkPolicy,
        run: &mut Value,
    ) -> Result<Value, PlatformError> {
        let project = project.ok_or(PlatformError::Conflict(
            "contributor ratings require a project-linked task",
        ))?;
        let task_id = run["task_id"].as_str().ok_or(PlatformError::Storage)?;
        let revision = run["task_revision"]
            .as_i64()
            .ok_or(PlatformError::Storage)?;
        TaskTransitionRunStore::authorize(store, ctx, Some(project), task_id)?;
        let referenced =
            Self::preceding_review(run, &branch.repository_id, &branch.expected_head_sha)?;
        let review = referenced["id"]
            .as_str()
            .ok_or(PlatformError::Storage)?
            .to_owned();
        let row=store.client().query_opt("SELECT input,report,base_sha FROM task_pre_pr_reviews WHERE id=$1 AND task_id=$2 AND task_revision=$3 AND repository_id=$4 AND head_sha=$5 AND status='completed'",&[&review,&task_id,&revision,&branch.repository_id,&branch.expected_head_sha]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("contributor ratings require the preceding completed technical review of this exact code"))?;
        let context: Value = row.get(0);
        let technical: Value = row.get(1);
        let pinned_base: String = row.get(2);
        if referenced["base_sha"] != pinned_base {
            return Err(PlatformError::Conflict(
                "preceding review base does not match its immutable record",
            ));
        }
        let reviewed_base = context["base_branch"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let verified = TaskTransitionProvider::verify(
            store,
            ctx,
            Some(project),
            branch,
            Some(reviewed_base),
            policy,
        )?;
        if verified["base_sha"] != pinned_base {
            return Err(PlatformError::Conflict(
                "reviewed target branch changed before contributor assessment",
            ));
        }
        let base_sha = pinned_base.as_str();
        TaskPrePrReview::context(
            store,
            ctx,
            &crate::task_review_locator::TaskReviewLocator {
                project,
                task: task_id,
                revision,
                repository: &branch.repository_id,
                head: &branch.expected_head_sha,
                review_id: Some(&review),
            },
        )?;
        let (source, token) = TaskTransitionProvider::source(
            store,
            ctx,
            Some(project),
            &branch.repository_id,
            policy,
        )?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let commits = TaskContributorCode::capture(
            &client,
            source.connection.provider,
            &TaskTransitionProvider::root(&source)?,
            &token,
            base_sha,
            &branch.expected_head_sha,
            &context["files"],
        )?;
        let provider = match source.connection.provider {
            GitProvider::Github => "github",
            GitProvider::Gitlab => "gitlab",
        };
        let contributors = TaskContributorCode::contributors(
            store,
            project,
            &branch.repository_id,
            provider,
            &source.connection.host,
            source
                .repository
                .remote_id
                .as_deref()
                .ok_or(PlatformError::Unavailable)?,
            &commits,
        )?;
        if contributors.is_empty() {
            return Ok(json!({"status":"completed","generated_count":0,"unrated_count":0}));
        }
        let mut generated = 0;
        let mut unrated = 0;
        let mut pending = Vec::new();
        for contributor in &contributors {
            let actor = contributor["actor_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let input = json!({"rubric_version":1,"review_id":review,"task":context["task"],"head_sha":context["head_sha"],"base_sha":context["base_sha"],"criteria":context["criteria"],"organization_knowledge":context["organization_knowledge"],"primary_knowledge":context["primary_knowledge"],"technical_review":technical,"contributor":actor,"commits":contributor["commits"],"attribution":contributor["attribution"]});
            let bytes = serde_json::to_vec(&input).map_err(|_| PlatformError::Storage)?;
            if bytes.len() > 450000 {
                return Err(PlatformError::Validation(
                    "private contributor evidence exceeds budget",
                ));
            }
            let fingerprint = format!("{:x}", Sha256::digest(&bytes));
            let (id, previous) = Self::claim(store, &review, actor, &fingerprint, &input)?;
            match previous.as_deref() {
                Some("completed") | Some("completed_no_score") => generated += 1,
                Some("queued") | Some("running") => {
                    pending.push(id);
                    unrated += 1;
                }
                Some(_) => {
                    return Err(PlatformError::Conflict(
                        "private assessment requires a new technical review or reconciliation",
                    ));
                }
                None => {
                    pending.push(id);
                    unrated += 1;
                }
            }
        }
        if !pending.is_empty() {
            run["knowledge_jobs"] = json!(
                pending
                    .iter()
                    .map(|job_id| json!({"kind":"contributor_rating","job_id":job_id}))
                    .collect::<Vec<_>>()
            );
            TaskTransitionRunStore::save(store, run)?;
            return Err(PlatformError::Conflict("waiting for knowledge worker"));
        }
        Ok(json!({"status":"completed","generated_count":generated,"unrated_count":unrated}))
    }
    fn preceding_review<'a>(
        run: &'a Value,
        repository: &str,
        head: &str,
    ) -> Result<&'a Value, PlatformError> {
        run["outcomes"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .filter(|step| step["kind"] == "create_code_review")
            .filter_map(|step| step["result"]["reviews"].as_array())
            .flat_map(|reviews| reviews.iter().rev())
            .find(|review| {
                review["repository_id"] == repository
                    && review["head_sha"] == head
                    && review["status"] == "completed"
            })
            .ok_or(PlatformError::Conflict(
                "contributor ratings require a preceding completed matching technical review",
            ))
    }
    pub fn context(
        store: &mut TaskWorkflowStore,
        task: &str,
        revision: i64,
        repository: &str,
        head: &str,
    ) -> Result<(), PlatformError> {
        let rows=store.client().query("SELECT g.actor_id,g.input,g.status FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.task_id=$1 AND r.task_revision=$2 AND r.repository_id=$3 AND r.head_sha=$4",&[&task,&revision,&repository,&head]).map_err(TaskWorkflowStore::database_error)?;
        for row in rows {
            let actor: String = row.get(0);
            let input: Value = row.get(1);
            let status: String = row.get(2);
            if !matches!(status.as_str(), "completed" | "completed_no_score") {
                return Err(PlatformError::Conflict(
                    "private contributor assessment is not reconciled",
                ));
            }
            for evidence in input["attribution"]
                .as_array()
                .ok_or(PlatformError::Storage)?
            {
                let alias = evidence["active_alias_id"]
                    .as_str()
                    .ok_or(PlatformError::Storage)?;
                let ar = evidence["alias_revision"]
                    .as_i64()
                    .ok_or(PlatformError::Storage)?;
                let rr = evidence["role_revision"]
                    .as_i64()
                    .ok_or(PlatformError::Storage)?;
                if store.client().query_opt("SELECT a.id FROM git_author_aliases a JOIN actors m ON m.id=a.actor_id AND m.active AND m.kind='human' JOIN actor_roles r ON r.actor_id=m.id AND r.role IN ('developer','technical_lead') AND r.revision=$4 WHERE a.id=$1 AND a.actor_id=$2 AND a.revision=$3 AND a.revoked_at IS NULL AND a.project_id IS NULL",&[&alias,&actor,&ar,&rr]).map_err(TaskWorkflowStore::database_error)?.is_none(){return Err(PlatformError::Conflict("contributor attribution changed before state advancement"))}
            }
        }
        Ok(())
    }
    fn claim(
        store: &mut TaskWorkflowStore,
        review: &str,
        actor: &str,
        fingerprint: &str,
        input: &Value,
    ) -> Result<(String, Option<String>), PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            &[&format!("private-rating:{review}:{actor}")],
        )
        .map_err(TaskWorkflowStore::database_error)?;
        if let Some(row) = tx
            .query_opt(
                "SELECT id,fingerprint,status FROM task_contributor_ratings WHERE review_id=$1 AND actor_id=$2 FOR UPDATE",
                &[&review, &actor],
            )
            .map_err(TaskWorkflowStore::database_error)?
        {
            let id: String = row.get(0);
            let previous_fingerprint: String = row.get(1);
            let status: String = row.get(2);
            tx.commit().map_err(TaskWorkflowStore::database_error)?;

            if previous_fingerprint != fingerprint {
                return Err(PlatformError::Conflict(
                    "private assessment evidence changed; obtain a new technical review",
                ));
            }

            if matches!(status.as_str(), "queued" | "running" | "completed" | "completed_no_score") {
                return Ok((id, Some(status)));
            }

            return Err(PlatformError::Conflict(
                "private assessment requires a new technical review or reconciliation",
            ));
        }
        let id = SecretGenerator::id("tgr")?;
        tx.execute("INSERT INTO task_contributor_ratings(id,review_id,actor_id,fingerprint,input,status) VALUES($1,$2,$3,$4,$5,'queued')",&[&id,&review,&actor,&fingerprint,&input]).map_err(TaskWorkflowStore::database_error)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok((id, None))
    }
    pub(crate) fn finish(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        completion: crate::task_contributor_rating_completion::TaskContributorRatingCompletion<'_>,
    ) -> Result<(bool, Option<i64>), PlatformError> {
        let crate::task_contributor_rating_completion::TaskContributorRatingCompletion {
            id,
            claim_token,
            review,
            actor,
            project,
            input,
            report,
            model,
        } = completion;
        let score = ContributorRatingReport::score(report, input)?;
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let reviewed=tx.query_one("SELECT task_id,task_revision,repository_id,authority FROM task_pre_pr_reviews WHERE id=$1 FOR SHARE",&[&review]).map_err(TaskWorkflowStore::database_error)?;
        let task_id: String = reviewed.get(0);
        let revision: i64 = reviewed.get(1);
        let repository: String = reviewed.get(2);
        let authority: Value = reviewed.get(3);
        let raw:Value=tx.query_opt("SELECT to_jsonb(t) FROM tasks t WHERE id=$1 AND project_id=$2 AND revision=$3 FOR UPDATE",&[&task_id,&project,&revision]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("task changed before personal assessment persistence"))?.get(0);
        let task: WorkTask = TaskWorkflowStore::decode(raw)?;
        crate::task_scope_authority::TaskScopeAuthority::require(
            &mut tx,
            ctx,
            Capability::TasksTransition,
            Some(project),
        )?;
        crate::task_scope_authority::TaskScopeAuthority::relation(
            &mut tx,
            ctx,
            Some(project),
            &task_id,
        )?;
        if crate::task_transition_provider_authority::TaskTransitionProviderAuthority::lock(
            &mut tx,
            ctx,
            project,
            &repository,
        )? != authority
        {
            return Err(PlatformError::Conflict(
                "repository authority changed before personal assessment persistence",
            ));
        }
        crate::organization_knowledge_authority::OrganizationKnowledgeAuthority::read(
            &mut tx, ctx,
        )?;
        tx.batch_execute("LOCK TABLE organization_documents,project_documents,project_document_versions,document_version_states,document_publications,project_features,requirement_features,feature_proposals IN SHARE MODE").map_err(TaskWorkflowStore::database_error)?;
        let docs:Vec<Value>=tx.query("SELECT jsonb_build_object('id',d.id,'revision',v.revision,'kind',d.kind,'title',v.title,'body',v.body) FROM organization_documents d JOIN organization_document_versions v ON v.document_id=d.id AND v.revision=d.published_revision WHERE d.kind IN ('security','best_practice') ORDER BY d.id FOR SHARE OF d,v",&[]).map_err(TaskWorkflowStore::database_error)?.into_iter().map(|r|r.get(0)).collect();
        if json!(docs) != input["organization_knowledge"]
            || crate::task_review_knowledge::TaskReviewKnowledge::capture_tx(&mut tx, ctx, &task)?
                != input["primary_knowledge"]
        {
            return Err(PlatformError::Conflict(
                "review knowledge changed before personal assessment persistence",
            ));
        }
        crate::git_author_policy::GitAuthorPolicy::lock(&mut tx)?;
        for evidence in input["attribution"]
            .as_array()
            .ok_or(PlatformError::Storage)?
        {
            let alias = evidence["active_alias_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let ar = evidence["alias_revision"]
                .as_i64()
                .ok_or(PlatformError::Storage)?;
            let rr = evidence["role_revision"]
                .as_i64()
                .ok_or(PlatformError::Storage)?;
            if tx.query_opt("SELECT a.id FROM git_author_aliases a JOIN actors m ON m.id=a.actor_id AND m.active AND m.kind='human' JOIN actor_roles r ON r.actor_id=m.id AND r.role IN ('developer','technical_lead') AND r.revision=$4 WHERE a.id=$1 AND a.actor_id=$2 AND a.revision=$3 AND a.revoked_at IS NULL AND a.project_id IS NULL FOR SHARE OF a,m,r",&[&alias,&actor,&ar,&rr]).map_err(TaskWorkflowStore::database_error)?.is_none(){return Err(PlatformError::Conflict("contributor alias or eligible role changed"))}
        }
        let row=tx.query_opt("SELECT g.status FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.id=$1 AND g.review_id=$2 AND g.actor_id=$3 AND g.status='running' AND g.claim_token=$4 AND g.lease_until>clock_timestamp() AND r.status='completed' FOR UPDATE OF g",&[&id,&review,&actor,&claim_token]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("private rating claim was superseded"))?;
        if row.get::<_, String>(0) != "running" {
            return Err(PlatformError::Conflict(
                "private rating claim is no longer running",
            ));
        }
        let entry = score.map(|_| format!("contributor-ai:{id}"));
        if let Some(score) = score {
            let inserted=tx.execute("INSERT INTO developer_rating_entries(id,actor_id,date,source,score,reason,created_by,project_id) SELECT $1,$2,(r.created_at AT TIME ZONE s.timezone)::date,'ai',$3,$4,NULL,$5 FROM task_pre_pr_reviews r CROSS JOIN labor_calendar_settings s WHERE r.id=$6 AND s.singleton",&[&entry,&actor,&score,&report["reason"].as_str().ok_or(PlatformError::Storage)?,&project,&review]).map_err(TaskWorkflowStore::database_error)?;
            if inserted != 1 {
                return Err(PlatformError::Conflict(
                    "organization calendar settings are unavailable",
                ));
            }
        }
        let status = if score.is_some() {
            "completed"
        } else {
            "completed_no_score"
        };
        let completed = tx
            .execute(
                crate::task_knowledge_job_api::COMPLETE_RATING,
                &[&id, &status, &report, &entry, &model, &claim_token],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        if completed != 1 {
            return Err(PlatformError::Conflict(
                "private rating claim was superseded",
            ));
        }
        let run_id: String = tx
            .query_one(
                "SELECT run_id FROM task_pre_pr_reviews WHERE id=$1",
                &[&review],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        let transition_actor: String = tx
            .query_one(
                "SELECT actor_id FROM task_transition_runs WHERE id=$1",
                &[&run_id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        let wake_generation = crate::task_knowledge_job_api::advance_knowledge_transition(
            &mut tx,
            &run_id,
            &transition_actor,
        )?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok((score.is_some(), wake_generation))
    }
}
