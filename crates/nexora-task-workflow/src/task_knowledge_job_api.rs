use crate::{
    TaskKnowledgeCompletion, TaskKnowledgeRetry, TaskWorkflowStore,
    task_knowledge_claim_record::TaskKnowledgeClaimRecord, task_knowledge_job::TaskKnowledgeJob,
    task_knowledge_wakeup::TaskKnowledgeWakeup,
};
use nexora_platform::PlatformError;
use serde_json::Value;

const CLAIM_REVIEW: &str = "UPDATE task_pre_pr_reviews SET status='running',lease_until=clock_timestamp()+interval '2 minutes',claim_token=gen_random_uuid()::text WHERE id=$1 AND ((status='queued' AND available_at<=clock_timestamp()) OR (status='running' AND lease_until<=clock_timestamp())) RETURNING id,run_id,task_id,input,claim_token";
const CLAIM_RATING: &str = "UPDATE task_contributor_ratings g SET status='running',lease_until=clock_timestamp()+interval '2 minutes',claim_token=gen_random_uuid()::text FROM task_pre_pr_reviews r WHERE g.id=$1 AND ((g.status='queued' AND g.available_at<=clock_timestamp()) OR (g.status='running' AND g.lease_until<=clock_timestamp())) AND r.id=g.review_id AND r.status='completed' RETURNING g.id,r.run_id,r.task_id,g.input,g.claim_token";
const COMPLETE_REVIEW: &str = "UPDATE task_pre_pr_reviews SET status='completed',report=$3,model=$4,lease_until=NULL,claim_token=NULL,finished_at=clock_timestamp() WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp()";
pub(crate) const COMPLETE_RATING: &str = "UPDATE task_contributor_ratings SET status=$2,report=$3,rating_entry_id=$4,model=$5,lease_until=NULL,claim_token=NULL,finished_at=clock_timestamp() WHERE id=$1 AND status='running' AND claim_token=$6 AND lease_until>clock_timestamp()";
const MAX_TASK_KNOWLEDGE_RETRIES: i32 = 10;
const RETRY_TASK_REVIEW: &str = "UPDATE task_pre_pr_reviews SET retry_count=retry_count+1,status=CASE WHEN retry_count+1 >= $5 THEN 'failed' ELSE 'queued' END,available_at=clock_timestamp()+($6::double precision*interval '1 second'),lease_until=NULL,claim_token=NULL,last_error_code=$3,last_error_message=$4,finished_at=CASE WHEN retry_count+1 >= $5 THEN clock_timestamp() ELSE NULL END WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp() RETURNING retry_count,available_at,status";
const FAIL_TASK_REVIEW: &str = "UPDATE task_pre_pr_reviews SET status='failed',lease_until=NULL,claim_token=NULL,last_error_code=$3,last_error_message=$4,finished_at=clock_timestamp() WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp()";
const RETRY_CONTRIBUTOR_RATING: &str = "UPDATE task_contributor_ratings SET retry_count=retry_count+1,status=CASE WHEN retry_count+1 >= $5 THEN 'failed' ELSE 'queued' END,available_at=clock_timestamp()+($6::double precision*interval '1 second'),lease_until=NULL,claim_token=NULL,last_error_code=$3,last_error_message=$4,finished_at=CASE WHEN retry_count+1 >= $5 THEN clock_timestamp() ELSE NULL END WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp() RETURNING retry_count,available_at,status";
const FAIL_CONTRIBUTOR_RATING: &str = "UPDATE task_contributor_ratings SET status='failed',lease_until=NULL,claim_token=NULL,last_error_code=$3,last_error_message=$4,finished_at=clock_timestamp() WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp()";
const FAILED_RATING_TRANSITION: &str = "UPDATE task_transition_runs tr SET record=tr.record||jsonb_build_object('status','failed','error_code','contributor_rating_failed','message','A private contributor assessment could not be completed.'),updated_at=clock_timestamp() WHERE tr.id=$1 AND tr.actor_id=$2 AND tr.record->>'status'='waiting_knowledge' AND EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status='failed') AND NOT EXISTS(SELECT 1 FROM task_pre_pr_reviews r WHERE r.run_id=tr.id AND r.status IN ('queued','running')) AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status IN ('queued','running'))";
pub(crate) const READY_KNOWLEDGE_TRANSITION: &str = "UPDATE task_transition_runs tr SET record=tr.record||jsonb_build_object('status','ready','error_code',NULL,'message',NULL),wake_pending=true,wake_generation=wake_generation+1,updated_at=clock_timestamp() WHERE tr.id=$1 AND tr.actor_id=$2 AND tr.record->>'status'='waiting_knowledge' AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status='failed') AND NOT EXISTS(SELECT 1 FROM task_pre_pr_reviews r WHERE r.run_id=tr.id AND r.status IN ('queued','running')) AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status IN ('queued','running')) RETURNING wake_generation";
const PENDING_KNOWLEDGE_JOBS: &str = "SELECT id,'task_review'::text AS job_kind,status,run_id,task_id,available_at FROM task_pre_pr_reviews WHERE status='queued' UNION ALL SELECT id,'task_review'::text,status,run_id,task_id,lease_until FROM task_pre_pr_reviews WHERE status='running' UNION ALL SELECT g.id,'contributor_rating'::text,g.status,r.run_id,r.task_id,g.available_at FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.status='queued' UNION ALL SELECT g.id,'contributor_rating'::text,g.status,r.run_id,r.task_id,g.lease_until FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.status='running' ORDER BY job_kind,run_id,task_id,id";
const KNOWLEDGE_WAKEUP_BY_ID: &str = "SELECT id,'task_review'::text AS job_kind,status,run_id,task_id,CASE WHEN status='running' THEN lease_until ELSE available_at END FROM task_pre_pr_reviews WHERE id=$1 AND status IN ('queued','running') UNION ALL SELECT g.id,'contributor_rating'::text,g.status,r.run_id,r.task_id,CASE WHEN g.status='running' THEN g.lease_until ELSE g.available_at END FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.id=$1 AND g.status IN ('queued','running')";

impl TaskWorkflowStore {
    /// Lists durable AI work for the API's existing RabbitMQ wakeup dispatcher.
    pub fn pending_task_knowledge_wakeups(
        &mut self,
    ) -> Result<Vec<TaskKnowledgeWakeup>, PlatformError> {
        self.client()
            .execute(
                "UPDATE task_transition_runs tr SET record=record||jsonb_build_object('status','failed','error_code','contributor_rating_failed','message','A private contributor assessment could not be completed.'),updated_at=clock_timestamp() WHERE tr.record->>'status'='waiting_knowledge' AND EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status='failed') AND NOT EXISTS(SELECT 1 FROM task_pre_pr_reviews r WHERE r.run_id=tr.id AND r.status IN ('queued','running')) AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status IN ('queued','running'))",
                &[],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        self.client()
            .execute(
                "UPDATE task_transition_runs tr SET record=record||jsonb_build_object('status','ready','error_code',NULL,'message',NULL),wake_pending=true,wake_generation=wake_generation+1,updated_at=clock_timestamp() WHERE tr.record->>'status'='waiting_knowledge' AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status='failed') AND NOT EXISTS(SELECT 1 FROM task_pre_pr_reviews r WHERE r.run_id=tr.id AND r.status IN ('queued','running')) AND NOT EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=tr.id AND g.status IN ('queued','running'))",
                &[],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        let rows = self
            .client()
            .query(PENDING_KNOWLEDGE_JOBS, &[])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(rows
            .into_iter()
            .map(|row| TaskKnowledgeWakeup {
                job_id: row.get(0),
                job_kind: row.get(1),
                status: row.get(2),
                run_id: row.get(3),
                task_id: row.get(4),
                available_at: row.get(5),
            })
            .collect())
    }

    /// Looks up a single durable job so a worker can schedule the lease-expiry retry after a lost wake.
    pub fn task_knowledge_wakeup(
        &mut self,
        job_id: &str,
    ) -> Result<Option<TaskKnowledgeWakeup>, PlatformError> {
        let row = self
            .client()
            .query_opt(KNOWLEDGE_WAKEUP_BY_ID, &[&job_id])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(row.map(|row| TaskKnowledgeWakeup {
            job_id: row.get(0),
            job_kind: row.get(1),
            status: row.get(2),
            run_id: row.get(3),
            task_id: row.get(4),
            available_at: row.get(5),
        }))
    }

    /// Claims a technical review with a fresh fencing token and a two-minute lease.
    pub fn claim_task_pre_pr_review(
        &mut self,
        job_id: &str,
    ) -> Result<Option<TaskKnowledgeJob>, PlatformError> {
        let row = self
            .client()
            .query_opt(CLAIM_REVIEW, &[&job_id])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(row.map(|row| TaskKnowledgeJob {
            job_id: row.get(0),
            job_kind: "task_review".to_owned(),
            run_id: row.get(1),
            task_id: row.get(2),
            input: row.get(3),
            claim_token: row.get(4),
        }))
    }

    /// Claims a contributor assessment with a fresh fencing token and a two-minute lease.
    pub fn claim_task_contributor_rating(
        &mut self,
        job_id: &str,
    ) -> Result<Option<TaskKnowledgeJob>, PlatformError> {
        let row = self
            .client()
            .query_opt(CLAIM_RATING, &[&job_id])
            .map_err(TaskWorkflowStore::database_error)?;
        Ok(row.map(|row| TaskKnowledgeJob {
            job_id: row.get(0),
            job_kind: "contributor_rating".to_owned(),
            run_id: row.get(1),
            task_id: row.get(2),
            input: row.get(3),
            claim_token: row.get(4),
        }))
    }

    /// Requeues transient knowledge failures with durable exponential backoff.
    pub fn retry_task_pre_pr_review(
        &mut self,
        job_id: &str,
        claim_token: &str,
        error_code: &str,
        message: &str,
    ) -> Result<TaskKnowledgeRetry, PlatformError> {
        let claim = self.task_review_claim_record(job_id, claim_token)?;
        self.validate_knowledge_actor(&claim.run_id, &claim.actor_id)?;
        let attempt = claim.retry_count + 1;
        let delay = retry_delay_seconds(attempt);
        let mut tx = self
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let row = tx
            .query_opt(
                RETRY_TASK_REVIEW,
                &[
                    &job_id,
                    &claim_token,
                    &error_code,
                    &message,
                    &MAX_TASK_KNOWLEDGE_RETRIES,
                    &delay,
                ],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict(
                "task knowledge claim was superseded",
            ))?;
        let retry_count: i32 = row.get(0);
        let available_at = row.get(1);
        let terminal = row.get::<_, String>(2) == "failed";
        if terminal {
            let failed = tx.execute(
                "UPDATE task_transition_runs SET record=record||jsonb_build_object('status','failed','error_code','knowledge_retry_exhausted','message',$2),updated_at=clock_timestamp() WHERE id=$1 AND actor_id=$3 AND record->>'status'='waiting_knowledge'",
                &[&claim.run_id, &message, &claim.actor_id],
            ).map_err(TaskWorkflowStore::database_error)?;
            if failed != 1 {
                return Err(PlatformError::Conflict(
                    "task transition is no longer waiting for knowledge",
                ));
            }
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(TaskKnowledgeRetry {
            run_id: claim.run_id,
            task_id: claim.task_id,
            retry_count,
            available_at,
            terminal,
            transition_failed: terminal,
        })
    }

    /// Permanently fails a claimed job and its transition in one transaction.
    pub fn fail_task_pre_pr_review(
        &mut self,
        job_id: &str,
        claim_token: &str,
        error_code: &str,
        message: &str,
    ) -> Result<crate::TaskKnowledgeFailure, PlatformError> {
        let claim = self.task_review_claim_record(job_id, claim_token)?;
        self.validate_knowledge_actor(&claim.run_id, &claim.actor_id)?;
        let mut tx = self
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let failed_job = tx
            .execute(
                FAIL_TASK_REVIEW,
                &[&job_id, &claim_token, &error_code, &message],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        if failed_job != 1 {
            return Err(PlatformError::Conflict(
                "task knowledge claim was superseded",
            ));
        }
        let failed_run = tx.execute(
            "UPDATE task_transition_runs SET record=record||jsonb_build_object('status','failed','error_code',$2,'message',$3),updated_at=clock_timestamp() WHERE id=$1 AND actor_id=$4 AND record->>'status'='waiting_knowledge'",
            &[&claim.run_id, &error_code, &message, &claim.actor_id],
        ).map_err(TaskWorkflowStore::database_error)?;
        if failed_run != 1 {
            return Err(PlatformError::Conflict(
                "task transition is no longer waiting for knowledge",
            ));
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(crate::TaskKnowledgeFailure {
            run_id: claim.run_id,
            transition_failed: true,
        })
    }

    /// Requeues private contributor model work under the current lease fence.
    /// Exhaustion fails the transition only after every other knowledge job settles.
    pub fn retry_task_contributor_rating(
        &mut self,
        job_id: &str,
        claim_token: &str,
        error_code: &str,
        private_message: &str,
    ) -> Result<TaskKnowledgeRetry, PlatformError> {
        let claim = self.contributor_rating_claim_record(job_id, claim_token)?;
        self.validate_knowledge_actor(&claim.run_id, &claim.actor_id)?;
        let attempt = claim.retry_count + 1;
        let delay = retry_delay_seconds(attempt);
        let mut tx = self
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let row = tx
            .query_opt(
                RETRY_CONTRIBUTOR_RATING,
                &[
                    &job_id,
                    &claim_token,
                    &error_code,
                    &private_message,
                    &MAX_TASK_KNOWLEDGE_RETRIES,
                    &delay,
                ],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict(
                "contributor assessment claim was superseded",
            ))?;
        let retry_count: i32 = row.get(0);
        let available_at = row.get(1);
        let terminal = row.get::<_, String>(2) == "failed";
        let transition_failed = if terminal {
            finalize_failed_private_rating(&mut tx, &claim)?
        } else {
            false
        };
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(TaskKnowledgeRetry {
            run_id: claim.run_id,
            task_id: claim.task_id,
            retry_count,
            available_at,
            terminal,
            transition_failed,
        })
    }

    /// Permanently fails private model work without exposing its diagnostics on the transition.
    pub fn fail_task_contributor_rating(
        &mut self,
        job_id: &str,
        claim_token: &str,
        error_code: &str,
        private_message: &str,
    ) -> Result<crate::TaskKnowledgeFailure, PlatformError> {
        let claim = self.contributor_rating_claim_record(job_id, claim_token)?;
        self.validate_knowledge_actor(&claim.run_id, &claim.actor_id)?;
        let mut tx = self
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let failed_job = tx
            .execute(
                FAIL_CONTRIBUTOR_RATING,
                &[&job_id, &claim_token, &error_code, &private_message],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        if failed_job != 1 {
            return Err(PlatformError::Conflict(
                "contributor assessment claim was superseded",
            ));
        }
        let transition_failed = finalize_failed_private_rating(&mut tx, &claim)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(crate::TaskKnowledgeFailure {
            run_id: claim.run_id,
            transition_failed,
        })
    }

    fn contributor_rating_claim_record(
        &mut self,
        job_id: &str,
        claim_token: &str,
    ) -> Result<TaskKnowledgeClaimRecord, PlatformError> {
        let row = self
            .client()
            .query_opt(
                "SELECT r.run_id,r.task_id,r.requested_by,g.retry_count FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.id=$1 AND g.status='running' AND g.claim_token=$2 AND g.lease_until>clock_timestamp()",
                &[&job_id, &claim_token],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict(
                "contributor assessment claim was superseded",
            ))?;
        Ok(TaskKnowledgeClaimRecord {
            run_id: row.get(0),
            task_id: row.get(1),
            actor_id: row.get(2),
            retry_count: row.get(3),
        })
    }

    fn task_review_claim_record(
        &mut self,
        job_id: &str,
        claim_token: &str,
    ) -> Result<TaskKnowledgeClaimRecord, PlatformError> {
        let sql = "SELECT run_id,task_id,requested_by,retry_count FROM task_pre_pr_reviews WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp()";
        let row = self
            .client()
            .query_opt(sql, &[&job_id, &claim_token])
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict(
                "task knowledge claim was superseded",
            ))?;
        Ok(TaskKnowledgeClaimRecord {
            run_id: row.get(0),
            task_id: row.get(1),
            actor_id: row.get(2),
            retry_count: row.get(3),
        })
    }

    fn validate_knowledge_actor(
        &mut self,
        run_id: &str,
        actor_id: &str,
    ) -> Result<(), PlatformError> {
        let row = self.client().query_opt(
            "SELECT credential_id,auth_method FROM task_transition_runs WHERE id=$1 AND actor_id=$2",
            &[&run_id, &actor_id],
        ).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        let credential_id: String = row.get(0);
        let method = match row.get::<_, String>(1).as_str() {
            "session" => nexora_platform::AuthMethod::Session,
            "api_key" => nexora_platform::AuthMethod::ApiKey,
            _ => return Err(PlatformError::Storage),
        };
        let current = crate::task_transition_authority::TaskTransitionAuthority::reload_credential(
            self,
            method,
            &credential_id,
        )?;
        if current.actor_id() != actor_id {
            return Err(PlatformError::Forbidden);
        }
        Ok(())
    }

    /// Validates current authority/context before atomically persisting the review and readying its run.
    pub fn complete_task_pre_pr_review(
        &mut self,
        job_id: &str,
        claim_token: &str,
        report: &Value,
        model: &str,
    ) -> Result<TaskKnowledgeCompletion, PlatformError> {
        let row = self.client().query_opt(
            "SELECT id,run_id,input,task_id,task_revision,project_id,repository_id,head_sha,requested_by FROM task_pre_pr_reviews WHERE id=$1 AND status='running' AND claim_token=$2 AND lease_until>clock_timestamp()",
            &[&job_id, &claim_token],
        ).map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict("task review claim was superseded"))?;
        let review_id: String = row.get(0);
        let run_id: String = row.get(1);
        let input: Value = row.get(2);
        let task_id: String = row.get(3);
        let revision: i64 = row.get(4);
        let project: String = row.get(5);
        let repository: String = row.get(6);
        let head: String = row.get(7);
        let actor: String = row.get(8);
        nexora_platform::TaskCodeReviewReport::validate(report, &input)?;

        let credential = self.client().query_one(
            "SELECT credential_id,auth_method FROM task_transition_runs WHERE id=$1 AND actor_id=$2",
            &[&run_id, &actor],
        ).map_err(TaskWorkflowStore::database_error)?;
        let credential_id: String = credential.get(0);
        let method = match credential.get::<_, String>(1).as_str() {
            "session" => nexora_platform::AuthMethod::Session,
            "api_key" => nexora_platform::AuthMethod::ApiKey,
            _ => return Err(PlatformError::Storage),
        };
        let context = crate::task_transition_authority::TaskTransitionAuthority::reload_credential(
            self,
            method,
            &credential_id,
        )?;
        if context.actor_id() != actor {
            return Err(PlatformError::Forbidden);
        }
        crate::task_pre_pr_review::TaskPrePrReview::context_for_completion(
            self,
            &context,
            &crate::task_review_locator::TaskReviewLocator {
                project: &project,
                task: &task_id,
                revision,
                repository: &repository,
                head: &head,
                review_id: Some(&review_id),
            },
            claim_token,
        )?;

        let mut tx = self
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let completed = tx
            .execute(COMPLETE_REVIEW, &[&job_id, &claim_token, report, &model])
            .map_err(TaskWorkflowStore::database_error)?;
        if completed != 1 {
            return Err(PlatformError::Conflict("task review claim was superseded"));
        }
        let wake_generation = advance_knowledge_transition(&mut tx, &run_id, &actor)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(TaskKnowledgeCompletion {
            run_id,
            transition_ready: wake_generation.is_some(),
            wake_generation,
        })
    }

    /// Persists the rating through the existing authority/revision checks and fencing token.
    pub fn complete_task_contributor_rating(
        &mut self,
        job_id: &str,
        claim_token: &str,
        report: &Value,
        model: &str,
    ) -> Result<TaskKnowledgeCompletion, PlatformError> {
        let row = self.client().query_opt(
            "SELECT g.review_id,g.actor_id,g.input,r.project_id FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE g.id=$1 AND g.status='running' AND g.claim_token=$2 AND g.lease_until>clock_timestamp()",
            &[&job_id, &claim_token],
        ).map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Conflict("contributor assessment claim was superseded"))?;
        let review: String = row.get(0);
        let actor: String = row.get(1);
        let input: Value = row.get(2);
        let project: String = row.get(3);
        let run_id: String = self
            .client()
            .query_one(
                "SELECT run_id FROM task_pre_pr_reviews WHERE id=$1",
                &[&review],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        let credential = self.client().query_one(
            "SELECT credential_id,auth_method FROM task_transition_runs WHERE id=$1 AND actor_id=$2",
            &[&run_id, &actor],
        ).map_err(TaskWorkflowStore::database_error)?;
        let credential_id: String = credential.get(0);
        let method = match credential.get::<_, String>(1).as_str() {
            "session" => nexora_platform::AuthMethod::Session,
            "api_key" => nexora_platform::AuthMethod::ApiKey,
            _ => return Err(PlatformError::Storage),
        };
        let context = crate::task_transition_authority::TaskTransitionAuthority::reload_credential(
            self,
            method,
            &credential_id,
        )?;
        if context.actor_id() != actor {
            return Err(PlatformError::Forbidden);
        }
        let (_, wake_generation) = crate::task_contributor_rating::TaskContributorRating::finish(
            self,
            &context,
            crate::task_contributor_rating_completion::TaskContributorRatingCompletion {
                id: job_id,
                claim_token,
                review: &review,
                actor: &actor,
                project: &project,
                input: &input,
                report,
                model,
            },
        )?;
        Ok(TaskKnowledgeCompletion {
            run_id,
            transition_ready: wake_generation.is_some(),
            wake_generation,
        })
    }
}

fn finalize_failed_private_rating(
    tx: &mut postgres::Transaction<'_>,
    claim: &TaskKnowledgeClaimRecord,
) -> Result<bool, PlatformError> {
    let failed = tx
        .execute(FAILED_RATING_TRANSITION, &[&claim.run_id, &claim.actor_id])
        .map_err(TaskWorkflowStore::database_error)?;
    if failed == 1 {
        return Ok(true);
    }
    let pending: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM task_pre_pr_reviews r WHERE r.run_id=$1 AND r.status IN ('queued','running')) OR EXISTS(SELECT 1 FROM task_contributor_ratings g JOIN task_pre_pr_reviews r ON r.id=g.review_id WHERE r.run_id=$1 AND g.status IN ('queued','running'))",
            &[&claim.run_id],
        )
        .map_err(TaskWorkflowStore::database_error)?
        .get(0);
    if pending {
        return Ok(false);
    }
    Err(PlatformError::Conflict(
        "task transition is no longer waiting for knowledge",
    ))
}

pub(crate) fn advance_knowledge_transition(
    tx: &mut postgres::Transaction<'_>,
    run_id: &str,
    actor_id: &str,
) -> Result<Option<i64>, PlatformError> {
    tx.execute(FAILED_RATING_TRANSITION, &[&run_id, &actor_id])
        .map_err(TaskWorkflowStore::database_error)?;
    tx.query_opt(READY_KNOWLEDGE_TRANSITION, &[&run_id, &actor_id])
        .map(|row| row.map(|row| row.get(0)))
        .map_err(TaskWorkflowStore::database_error)
}

fn retry_delay_seconds(attempt: i32) -> i64 {
    5_i64
        .saturating_mul(
            1_i64
                .checked_shl(attempt.saturating_sub(1).clamp(0, 8) as u32)
                .unwrap_or(256),
        )
        .min(900)
}

#[cfg(test)]
mod tests {
    use super::{
        CLAIM_RATING, CLAIM_REVIEW, COMPLETE_RATING, COMPLETE_REVIEW, FAIL_CONTRIBUTOR_RATING,
        FAIL_TASK_REVIEW, FAILED_RATING_TRANSITION, KNOWLEDGE_WAKEUP_BY_ID, PENDING_KNOWLEDGE_JOBS,
        READY_KNOWLEDGE_TRANSITION, RETRY_CONTRIBUTOR_RATING, RETRY_TASK_REVIEW,
    };

    #[test]
    fn each_reclaim_rotates_a_claim_fence() {
        for claim in [CLAIM_REVIEW, CLAIM_RATING] {
            assert!(claim.contains("claim_token=gen_random_uuid()::text"));
            assert!(claim.contains("lease_until<=clock_timestamp()"));
            assert!(claim.contains("available_at<=clock_timestamp()"));
            assert!(claim.contains("RETURNING") && claim.ends_with("claim_token"));
        }
    }

    #[test]
    fn terminal_updates_require_the_current_unexpired_fence_and_clear_it() {
        for completion in [COMPLETE_REVIEW, COMPLETE_RATING] {
            assert!(completion.contains("claim_token=$"));
            assert!(completion.contains("lease_until>clock_timestamp()"));
            assert!(completion.contains("claim_token=NULL"));
        }
    }

    #[test]
    fn retry_backoff_is_bounded_and_nonzero() {
        assert_eq!(super::retry_delay_seconds(1), 5);
        assert_eq!(super::retry_delay_seconds(2), 10);
        assert_eq!(super::retry_delay_seconds(9), 900);
        assert_eq!(super::retry_delay_seconds(30), 900);
    }

    #[test]
    fn review_retry_and_failure_require_the_live_fence_and_release_it() {
        for sql in [RETRY_TASK_REVIEW, FAIL_TASK_REVIEW] {
            assert!(sql.contains("claim_token=$2"));
            assert!(sql.contains("lease_until>clock_timestamp()"));
            assert!(sql.contains("claim_token=NULL"));
        }
        assert!(RETRY_TASK_REVIEW.contains("available_at=clock_timestamp()+"));
        assert!(RETRY_TASK_REVIEW.contains("retry_count=retry_count+1"));
    }

    #[test]
    fn rating_retry_and_failure_are_fenced_and_keep_diagnostics_private() {
        for sql in [RETRY_CONTRIBUTOR_RATING, FAIL_CONTRIBUTOR_RATING] {
            assert!(sql.contains("claim_token=$2"));
            assert!(sql.contains("lease_until>clock_timestamp()"));
            assert!(sql.contains("claim_token=NULL"));
            assert!(sql.contains("last_error_message=$4"));
        }
        assert!(RETRY_CONTRIBUTOR_RATING.contains("available_at=clock_timestamp()+"));
        assert!(RETRY_CONTRIBUTOR_RATING.contains("retry_count=retry_count+1"));
        assert!(FAILED_RATING_TRANSITION.contains("A private contributor assessment"));
        assert!(!FAILED_RATING_TRANSITION.contains("last_error_message"));
        assert!(!FAILED_RATING_TRANSITION.contains("g.report"));
    }

    #[test]
    fn transition_advances_only_after_all_jobs_settle_and_no_rating_failed() {
        for sql in [FAILED_RATING_TRANSITION, READY_KNOWLEDGE_TRANSITION] {
            assert!(sql.contains("status IN ('queued','running')"));
            assert!(sql.contains("g.status='failed'"));
        }
        assert!(READY_KNOWLEDGE_TRANSITION.contains("wake_pending=true"));
        assert!(READY_KNOWLEDGE_TRANSITION.contains("wake_generation=wake_generation+1"));
    }

    #[test]
    fn running_wakeups_expose_lease_expiry_for_one_shot_reclaim() {
        for sql in [PENDING_KNOWLEDGE_JOBS, KNOWLEDGE_WAKEUP_BY_ID] {
            assert!(sql.contains("status='running'"));
            assert!(sql.contains("lease_until"));
        }
        assert!(PENDING_KNOWLEDGE_JOBS.contains("g.lease_until"));
        assert!(PENDING_KNOWLEDGE_JOBS.contains("g.available_at"));
        assert!(KNOWLEDGE_WAKEUP_BY_ID.contains("CASE WHEN status='running'"));
    }
}
