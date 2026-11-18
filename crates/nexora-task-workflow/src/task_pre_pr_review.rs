use crate::{
    GitNetworkPolicy, SecretGenerator, TaskWorkflowStore, pull_request_client::PullRequestClient,
    task_transition_provider::TaskTransitionProvider,
    task_transition_run_store::TaskTransitionRunStore,
};
use nexora_platform::*;
use serde_json::{Value, json};
pub(crate) struct TaskPrePrReview;
impl TaskPrePrReview {
    fn knowledge(store: &mut TaskWorkflowStore, ctx: &AuthContext) -> Result<Value, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        crate::organization_knowledge_authority::OrganizationKnowledgeAuthority::read(
            &mut tx, ctx,
        )?;
        let rows=tx.query("SELECT jsonb_build_object('id',d.id,'revision',v.revision,'kind',d.kind,'title',v.title,'body',v.body) FROM organization_documents d JOIN organization_document_versions v ON v.document_id=d.id AND v.revision=d.published_revision WHERE d.kind IN ('security','best_practice') ORDER BY d.id LIMIT 65",&[]).map_err(TaskWorkflowStore::database_error)?;
        let docs: Vec<Value> = rows.into_iter().map(|r| r.get(0)).collect();
        if docs.len() > 64
            || serde_json::to_vec(&docs)
                .map_err(|_| PlatformError::Storage)?
                .len()
                > 196608
        {
            return Err(PlatformError::Validation(
                "global review knowledge exceeds review budget; consolidate organization policies",
            ));
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(json!(docs))
    }
    pub(crate) fn files(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        base: &str,
        head: &str,
    ) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let raw = client.get(
            &if github {
                format!("{root}/compare/{base}...{head}")
            } else {
                format!("{root}/repository/compare?from={base}&to={head}&straight=true")
            },
            token,
        )?;
        if raw["compare_timeout"] == true || raw["overflow"] == true {
            return Err(PlatformError::Validation("provider diff is incomplete"));
        }
        let entries = raw[if github { "files" } else { "diffs" }]
            .as_array()
            .ok_or(PlatformError::Unavailable)?;
        if entries.is_empty() || github && entries.len() >= 300 {
            return Err(PlatformError::Validation(
                "provider review diff is empty or reaches the provider file truncation limit",
            ));
        }
        let mut files = Vec::new();
        let mut size = 0;
        for entry in entries {
            let patch = entry[if github { "patch" } else { "diff" }]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or(PlatformError::Validation(
                    "binary or omitted provider diff cannot be fully reviewed",
                ))?;
            if entry["collapsed"] == true || entry["too_large"] == true {
                return Err(PlatformError::Validation("provider diff is truncated"));
            }
            if github {
                let additions = patch.lines().filter(|l| l.starts_with('+')).count() as u64;
                let deletions = patch.lines().filter(|l| l.starts_with('-')).count() as u64;
                if entry["additions"].as_u64() != Some(additions)
                    || entry["deletions"].as_u64() != Some(deletions)
                {
                    return Err(PlatformError::Validation("provider diff is truncated"));
                }
            }
            let path = entry[if github { "filename" } else { "new_path" }]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4096)
                .ok_or(PlatformError::Unavailable)?;
            size += patch.len();
            files.push(json!({"path":path,"patch":patch}));
        }
        if size > 192 * 1024 {
            return Err(PlatformError::Validation(
                "provider diff exceeds review budget",
            ));
        }
        Ok(json!(files))
    }
    /// Captures review context and persists a knowledge-worker job when needed.
    /// Model inference is intentionally owned by knowledge-worker.
    pub fn enqueue_or_load(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        branch: &TaskTransitionBranch,
        base: Option<&str>,
        policy: &GitNetworkPolicy,
        run: &mut Value,
    ) -> Result<Value, PlatformError> {
        let project = project.ok_or(PlatformError::Conflict(
            "code reviews require a project-linked task",
        ))?;
        let task = TaskTransitionRunStore::authorize(
            store,
            ctx,
            Some(project),
            run["task_id"].as_str().ok_or(PlatformError::Storage)?,
        )?;
        if Some(task.revision) != run["task_revision"].as_i64() {
            return Err(PlatformError::Conflict(
                "task revision changed before review",
            ));
        }
        let verified =
            TaskTransitionProvider::verify(store, ctx, Some(project), branch, base, policy)?;
        let base_sha = verified["base_sha"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let base_branch = verified["base_branch"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let authority =
            TaskTransitionProvider::authority(store, ctx, Some(project), &branch.repository_id)?;
        let knowledge = Self::knowledge(store, ctx)?;
        let primary_knowledge =
            crate::task_review_knowledge::TaskReviewKnowledge::capture(store, ctx, &task)?;
        let run_id = run["id"].as_str().ok_or(PlatformError::Storage)?;
        if let Some(row)=store.client().query_opt("SELECT id,status,report,input,authority FROM task_pre_pr_reviews WHERE run_id=$1 AND repository_id=$2 AND head_sha=$3 AND base_sha=$4 ORDER BY created_at DESC LIMIT 1",&[&run_id,&branch.repository_id,&branch.expected_head_sha,&base_sha]).map_err(TaskWorkflowStore::database_error)? {
            let status:String=row.get(1);let input:Value=row.get(3);let previous_authority:Value=row.get(4);
            if input["organization_knowledge"]!=knowledge||input["primary_knowledge"]!=primary_knowledge||previous_authority!=authority{return Err(PlatformError::Conflict("review knowledge or repository authority changed; obtain a new task revision and review"));}
            if status=="completed" {return Ok(json!({"id":row.get::<_,String>(0),"head_sha":branch.expected_head_sha,"base_sha":base_sha,"report":row.get::<_,Option<Value>>(2),"status":"completed"}));}
            if matches!(status.as_str(), "queued" | "running") {
                run["knowledge_jobs"] = json!([{"kind":"task_review","job_id":row.get::<_,String>(0)}]);
                TaskTransitionRunStore::save(store, run)?;
                return Err(PlatformError::Conflict("waiting for knowledge worker"));
            }
            if status != "failed" {
                return Err(PlatformError::Conflict("review became stale; refresh the task revision"));
            }
        }
        let (source, token) = TaskTransitionProvider::source(
            store,
            ctx,
            Some(project),
            &branch.repository_id,
            policy,
        )?;
        let client = PullRequestClient::new(policy, source.connection.provider);
        let files = Self::files(
            &client,
            source.connection.provider,
            &TaskTransitionProvider::root(&source)?,
            &token,
            base_sha,
            &branch.expected_head_sha,
        )?;
        if task.acceptance_criteria.len() > 64 {
            return Err(PlatformError::Validation(
                "too many acceptance criteria for a bounded review",
            ));
        }
        let mut criteria: Vec<_> = task
            .acceptance_criteria
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"id":format!("C{}",i+1),"description":c}))
            .collect();
        criteria.extend(crate::task_review_knowledge::TaskReviewKnowledge::criteria(
            &primary_knowledge,
        ));
        if criteria.len() > 64 {
            return Err(PlatformError::Validation(
                "combined task and requirement criteria exceed a single review output budget",
            ));
        }
        let input = json!({"task":{"id":task.id,"revision":task.revision,"title":task.title,"context":task.context},"head_sha":branch.expected_head_sha,"base_sha":base_sha,"base_branch":base_branch,"files":files,"criteria":criteria,"organization_knowledge":knowledge,"primary_knowledge":primary_knowledge});
        if serde_json::to_vec(&input)
            .map_err(|_| PlatformError::Storage)?
            .len()
            > 450000
        {
            return Err(PlatformError::Validation("review input exceeds budget"));
        }
        let id = SecretGenerator::id("tcr")?;
        store.client().execute(
            "INSERT INTO task_pre_pr_reviews(id,run_id,task_id,task_revision,project_id,repository_id,head_sha,base_sha,requested_by,authority,input,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'queued')",
            &[&id,&run_id,&task.id,&task.revision,&project,&branch.repository_id,&branch.expected_head_sha,&base_sha,&ctx.actor_id(),&authority,&input],
        ).map_err(TaskWorkflowStore::database_error)?;
        run["knowledge_jobs"] = json!([{"kind":"task_review","job_id":id}]);
        TaskTransitionRunStore::save(store, run)?;
        Err(PlatformError::Conflict("waiting for knowledge worker"))
    }
    pub fn context(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        locator: &crate::task_review_locator::TaskReviewLocator<'_>,
    ) -> Result<(), PlatformError> {
        Self::validate_context(store, ctx, locator, "completed", None)
    }

    pub(crate) fn context_for_completion(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        locator: &crate::task_review_locator::TaskReviewLocator<'_>,
        claim_token: &str,
    ) -> Result<(), PlatformError> {
        Self::validate_context(store, ctx, locator, "running", Some(claim_token))
    }

    fn validate_context(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        locator: &crate::task_review_locator::TaskReviewLocator<'_>,
        status: &str,
        claim_token: Option<&str>,
    ) -> Result<(), PlatformError> {
        let review_id = locator.review_id.ok_or(PlatformError::Storage)?;
        let row=store.client().query_opt("SELECT input,authority FROM task_pre_pr_reviews WHERE id=$1 AND task_id=$2 AND task_revision=$3 AND repository_id=$4 AND head_sha=$5 AND status=$6 AND ($7::text IS NULL OR claim_token=$7)",&[&review_id,&locator.task,&locator.revision,&locator.repository,&locator.head,&status,&claim_token]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("review claim or task revision changed"))?;
        let input: Value = row.get(0);
        let current =
            TaskTransitionRunStore::authorize(store, ctx, Some(locator.project), locator.task)?;
        if current.revision != locator.revision
            || input["primary_knowledge"]
                != crate::task_review_knowledge::TaskReviewKnowledge::capture(store, ctx, &current)?
        {
            return Err(PlatformError::Conflict(
                "primary requirement or feature changed after review",
            ));
        }
        let authority: Value = row.get(1);
        if input["organization_knowledge"] != Self::knowledge(store, ctx)?
            || authority
                != TaskTransitionProvider::authority(
                    store,
                    ctx,
                    Some(locator.project),
                    locator.repository,
                )?
        {
            return Err(PlatformError::Conflict(
                "code review context changed before state advancement",
            ));
        }
        Ok(())
    }
    pub fn description(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        locator: &crate::task_review_locator::TaskReviewLocator<'_>,
        base: &str,
    ) -> Result<String, PlatformError> {
        let row=store.client().query_opt("SELECT report,input,authority FROM task_pre_pr_reviews WHERE task_id=$1 AND task_revision=$2 AND repository_id=$3 AND head_sha=$4 AND base_sha=$5 AND status='completed' ORDER BY created_at DESC LIMIT 1",&[&locator.task,&locator.revision,&locator.repository,&locator.head,&base]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Conflict("pre-PR code review is required for this exact task and code revision"))?;
        let input: Value = row.get(1);
        let current =
            TaskTransitionRunStore::authorize(store, ctx, Some(locator.project), locator.task)?;
        if current.revision != locator.revision
            || input["primary_knowledge"]
                != crate::task_review_knowledge::TaskReviewKnowledge::capture(store, ctx, &current)?
        {
            return Err(PlatformError::Conflict(
                "primary requirement or feature changed before pull request creation",
            ));
        }
        if input["organization_knowledge"] != Self::knowledge(store, ctx)? {
            return Err(PlatformError::Conflict(
                "organization review knowledge changed before pull request creation",
            ));
        }
        if row.get::<_, Value>(2)
            != TaskTransitionProvider::authority(
                store,
                ctx,
                Some(locator.project),
                locator.repository,
            )?
        {
            return Err(PlatformError::Conflict(
                "repository authority changed before pull request creation",
            ));
        }
        let report: Value = row.get(0);
        let technical =
            serde_json::to_string_pretty(&report).map_err(|_| PlatformError::Storage)?;
        Ok(format!(
            "\n\n### Nexora pre-PR technical review\nStatic analysis only; no tests are claimed as executed.\n\n```json\n{technical}\n```"
        ))
    }
}
