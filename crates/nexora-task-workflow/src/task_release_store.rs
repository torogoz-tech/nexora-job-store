use crate::{
    TaskWorkflowStore, audit_writer::AuditWriter, platform_authority::PlatformAuthority,
    task_history_writer::TaskHistoryWriter, task_policy_store::TaskPolicyStore,
    task_scope_authority::TaskScopeAuthority,
};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::{Value, json};
pub(crate) struct TaskReleaseStore;
impl TaskReleaseStore {
    fn authority(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        write: bool,
    ) -> Result<(), PlatformError> {
        PlatformAuthority::require(
            tx,
            ctx,
            if write {
                Capability::TasksWrite
            } else {
                Capability::TasksRead
            },
            Some(project),
        )?;
        PlatformAuthority::project(tx, project)?;
        if write {
            tx.query_opt("SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','manager','super_admin') FOR SHARE",&[&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        }
        Ok(())
    }
    pub(crate) fn authorize_publication(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: &str,
    ) -> Result<(), PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        Self::authority(&mut tx, ctx, project, true)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)
    }
    pub(crate) fn validate_pins(
        store: &mut TaskWorkflowStore,
        project: &str,
        release: &str,
    ) -> Result<(), PlatformError> {
        let invalid:bool=store.client().query_one("SELECT EXISTS(SELECT 1 FROM task_release_items i LEFT JOIN tasks t ON t.id=i.task_id WHERE i.release_id=$1 AND (t.id IS NULL OR t.project_id IS DISTINCT FROM $2 OR t.revision<>i.task_revision OR t.archived OR t.lifecycle<>'active')) OR EXISTS(SELECT 1 FROM task_release_commit_pins p LEFT JOIN task_commit_links l ON l.task_id=p.task_id AND l.repository_id=p.repository_id AND l.commit_sha=p.commit_sha AND l.removed_at IS NULL WHERE p.release_id=$1 AND (l.id IS NULL OR l.provider_evidence->>'provenance' IS DISTINCT FROM 'provider_verified'))",&[&release,&project]).map_err(TaskWorkflowStore::database_error)?.get(0);
        if invalid {
            return Err(PlatformError::Conflict(
                "selected release task revisions or commit bindings changed",
            ));
        }
        Ok(())
    }
    fn projection(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
    ) -> Result<Value, PlatformError> {
        Ok(tx.query_opt("SELECT to_jsonb(r)||jsonb_build_object('tasks',COALESCE((SELECT jsonb_agg(to_jsonb(i) ORDER BY task_id) FROM task_release_items i WHERE release_id=r.id),'[]'::jsonb),'commits',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY task_id,sequence,repository_id,commit_sha) FROM task_release_commit_pins c WHERE release_id=r.id),'[]'::jsonb),'sources',COALESCE((SELECT jsonb_agg(to_jsonb(s) ORDER BY task_id,repository_id) FROM task_release_source_pins s WHERE release_id=r.id),'[]'::jsonb),'repositories',COALESCE((SELECT jsonb_agg(to_jsonb(i) ORDER BY repository_id) FROM task_release_repositories i WHERE release_id=r.id),'[]'::jsonb)) FROM task_releases r WHERE id=$1 AND project_id=$2",&[&id,&project]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?.get(0))
    }
    pub fn get(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: &str,
        id: &str,
    ) -> Result<Value, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        Self::authority(&mut tx, ctx, project, false)?;
        let value = Self::projection(&mut tx, project, id)?;
        for task in value["tasks"].as_array().ok_or(PlatformError::Storage)? {
            TaskScopeAuthority::task(
                &mut tx,
                ctx,
                task["task_id"].as_str().ok_or(PlatformError::Storage)?,
                Capability::TasksRead,
            )?;
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(value)
    }
    /// Called only after the transition executor independently verified production deployments.
    pub(crate) fn publish(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: &str,
        id: &str,
        revision: i64,
        verified_repositories: &[(String, String)],
        evidence: &Value,
    ) -> Result<Value, PlatformError> {
        let verified_facts = store.transition_verified_facts.clone();
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        crate::task_relation_lock::TaskRelationLock::acquire(&mut tx)?;
        Self::authority(&mut tx, ctx, project, true)?;
        let row=tx.query_opt("SELECT revision,status,created_by,publication_evidence FROM task_releases WHERE id=$1 AND project_id=$2 FOR UPDATE",&[&id,&project]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
        if row.get::<_, String>(1) == "published"
            && row.get::<_, i64>(0) == revision + 1
            && row.get::<_, Option<Value>>(3).is_some_and(|old| {
                old["transition_run_id"].is_string()
                    && old["transition_run_id"] == evidence["transition_run_id"]
            })
        {
            let value = Self::projection(&mut tx, project, id)?;
            tx.commit().map_err(TaskWorkflowStore::database_error)?;
            return Ok(value);
        }
        if row.get::<_, i64>(0) != revision || row.get::<_, String>(1) != "prepared" {
            return Err(PlatformError::Conflict(
                "release changed before production publication",
            ));
        }
        if tx.query_one("SELECT EXISTS(SELECT 1 FROM task_release_repositories WHERE release_id=$1 AND (composition_status<>'composed' OR head_sha IS NULL OR composition_evidence->>'provenance' IS DISTINCT FROM 'server_composed'))",&[&id]).map_err(TaskWorkflowStore::database_error)?.get::<_,bool>(0){return Err(PlatformError::Conflict("compose and verify the selected release commits before publication"));}
        let expected:Vec<(String,String)>=tx.query("SELECT repository_id,head_sha FROM task_release_repositories WHERE release_id=$1 ORDER BY repository_id",&[&id]).map_err(TaskWorkflowStore::database_error)?.into_iter().map(|r|(r.get(0),r.get(1))).collect();
        let mut verified = verified_repositories.to_vec();
        verified.sort();
        if expected.is_empty() || expected != verified {
            return Err(PlatformError::Conflict(
                "production verification must cover every exact release repository SHA",
            ));
        }
        for fact in &verified_facts {
            let repository = fact["repository_id"]
                .as_str()
                .ok_or(PlatformError::Storage)?;
            let current =
                crate::task_transition_provider_authority::TaskTransitionProviderAuthority::lock(
                    &mut tx, ctx, project, repository,
                )?;
            if &current != fact {
                return Err(PlatformError::Conflict(
                    "repository authority changed before release publication",
                ));
            }
        }
        let source_count: i64 = tx
            .query_one(
                "SELECT count(*) FROM task_release_source_pins WHERE release_id=$1",
                &[&id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .get(0);
        let source_rows=tx.query("SELECT b.head_sha,s.source_head_sha FROM task_release_source_pins s JOIN task_repository_branches b ON b.task_id=s.task_id AND b.repository_id=s.repository_id WHERE s.release_id=$1 ORDER BY s.task_id,s.repository_id FOR SHARE OF b",&[&id]).map_err(TaskWorkflowStore::database_error)?;
        if source_count == 0
            || source_rows.len() as i64 != source_count
            || source_rows
                .iter()
                .any(|row| row.get::<_, String>(0) != row.get::<_, String>(1))
        {
            return Err(PlatformError::Conflict(
                "task source branch changed after release selection",
            ));
        }
        let items=tx.query("WITH RECURSIVE ordered AS (SELECT i.task_id,i.task_revision,0 AS depth FROM task_release_items i WHERE i.release_id=$1 AND NOT EXISTS(SELECT 1 FROM task_subtasks s JOIN task_release_items p ON p.task_id=s.parent_task_id AND p.release_id=$1 WHERE s.child_task_id=i.task_id AND s.removed_at IS NULL) UNION ALL SELECT i.task_id,i.task_revision,p.depth+1 FROM task_release_items i JOIN task_subtasks s ON s.child_task_id=i.task_id AND s.removed_at IS NULL JOIN ordered p ON p.task_id=s.parent_task_id WHERE i.release_id=$1) SELECT task_id,task_revision FROM ordered ORDER BY depth DESC,task_id",&[&id]).map_err(TaskWorkflowStore::database_error)?;
        if items.is_empty() {
            return Err(PlatformError::Conflict("release has no selected tasks"));
        }
        for row in items {
            let task_id: String = row.get(0);
            let pinned: i64 = row.get(1);
            let before = TaskPolicyStore::scoped_task(&mut tx, Some(project), &task_id)?;
            if before.revision != pinned
                || before.archived
                || before.lifecycle != TaskLifecycle::Active
            {
                return Err(PlatformError::Conflict(
                    "release task changed before production publication",
                ));
            }
            crate::task_relation_policy::TaskRelationPolicy::state(&mut tx, &before, true)?;
            tx.execute("INSERT INTO task_production_publications(task_id,task_revision,release_id,evidence) VALUES($1,$2,$3,$4)",&[&task_id,&pinned,&id,evidence]).map_err(TaskWorkflowStore::database_error)?;
            let value:Value=tx.query_one("UPDATE tasks AS t SET lifecycle='completed',revision=revision+1,updated_at=clock_timestamp() WHERE id=$1 RETURNING to_jsonb(t)",&[&task_id]).map_err(TaskWorkflowStore::database_error)?.get(0);
            let task: WorkTask = TaskWorkflowStore::decode(value)?;
            TaskHistoryWriter::event(
                &mut tx,
                ctx,
                &task,
                "task.production_published",
                json!({"before":TaskHistoryWriter::state(&before)?,"after":TaskHistoryWriter::state(&task)?,"release_id":id,"provider_verification":evidence}),
            )?;
        }
        tx.execute("UPDATE task_releases SET status='published',revision=revision+1,published_at=clock_timestamp(),publication_evidence=$2 WHERE id=$1",&[&id,&evidence]).map_err(TaskWorkflowStore::database_error)?;
        let value = Self::projection(&mut tx, project, id)?;
        AuditWriter::append(
            &mut tx,
            Some(ctx.actor_id()),
            Some(ctx.credential_id()),
            "release.production_published",
            Some(id),
            json!({"project_id":project,"provider_verification":evidence}),
        )?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(value)
    }
}
