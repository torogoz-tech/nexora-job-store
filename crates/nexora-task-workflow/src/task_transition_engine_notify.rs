use super::TaskTransitionEngine;
use crate::TaskWorkflowStore;
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionEngine {
    pub(crate) fn notify(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        id: &str,
        index: usize,
        channel: &str,
        audience: &str,
        run: &Value,
    ) -> Result<Value, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let task = crate::task_scope_authority::TaskScopeAuthority::task(
            &mut tx,
            ctx,
            id,
            Capability::TasksTransition,
        )?;
        let key = format!(
            "workflow:{}:{index}",
            run["id"].as_str().ok_or(PlatformError::Storage)?
        );
        if audience == "approvers" {
            if channel == "email" {
                tx.execute("UPDATE email_notifications SET status='pending',attempts=0,available_at=clock_timestamp() WHERE strpos(event_key,$1||':')=1 AND status='failed'",&[&key]).map_err(TaskWorkflowStore::database_error)?;
                tx.execute("INSERT INTO email_notifications(event_key,kind,project_id,resource_id,actor_id) SELECT $1||':'||r.actor_id,'workflow_transition',$2,$3,r.actor_id FROM notification_recipients r JOIN actors a ON a.id=r.actor_id AND a.active AND a.kind='human' WHERE r.actor_id<>$4 AND r.email<>'' AND r.approvals AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=r.actor_id AND g.capability='approvals:grant' AND (g.project_id IS NULL OR g.project_id=$2)) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=r.actor_id AND g.capability='tasks:read' AND (g.project_id IS NULL OR g.project_id=$2)) AND EXISTS(SELECT 1 FROM actor_roles role WHERE role.actor_id=r.actor_id AND (role.role='super_admin' OR role.role IN ('technical_lead','pmo','manager') AND ($2::text IS NULL OR role.all_projects OR EXISTS(SELECT 1 FROM member_role_projects scope WHERE scope.actor_id=role.actor_id AND scope.project_id=$2)))) ON CONFLICT DO NOTHING",&[&key,&task.project_id,&id,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?;
            } else {
                tx.execute("INSERT INTO in_app_notifications(id,recipient_id,kind,project_id,resource_id) SELECT $1||':'||u.actor_id,u.actor_id,'task_updated',$2,$3 FROM users u JOIN actors a ON a.id=u.actor_id AND a.active AND a.kind='human' WHERE u.actor_id<>$4 AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=u.actor_id AND g.capability='approvals:grant' AND (g.project_id IS NULL OR g.project_id=$2)) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=u.actor_id AND g.capability='tasks:read' AND (g.project_id IS NULL OR g.project_id=$2)) AND EXISTS(SELECT 1 FROM actor_roles role WHERE role.actor_id=u.actor_id AND (role.role='super_admin' OR role.role IN ('technical_lead','pmo','manager') AND ($2::text IS NULL OR role.all_projects OR EXISTS(SELECT 1 FROM member_role_projects scope WHERE scope.actor_id=role.actor_id AND scope.project_id=$2)))) ON CONFLICT DO NOTHING",&[&key,&task.project_id,&id,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?;
            }
        } else if let Some(actor) = &task.assignee_id {
            if channel == "email" {
                tx.execute("UPDATE email_notifications SET status='pending',attempts=0,available_at=clock_timestamp() WHERE event_key=$1 AND status='failed'",&[&key]).map_err(TaskWorkflowStore::database_error)?;
                tx.execute("INSERT INTO email_notifications(event_key,kind,project_id,resource_id,actor_id) SELECT $1,'workflow_transition',$2,$3,$4 FROM notification_recipients r JOIN actors a ON a.id=r.actor_id AND a.active WHERE r.actor_id=$4 AND r.email<>'' AND r.assignments ON CONFLICT DO NOTHING",&[&key,&task.project_id,&id,&actor]).map_err(TaskWorkflowStore::database_error)?;
            } else {
                tx.execute("INSERT INTO in_app_notifications(id,recipient_id,kind,project_id,resource_id) SELECT $1,$2,'task_updated',$3,$4 FROM users u JOIN actors a ON a.id=u.actor_id AND a.active WHERE u.actor_id=$2 ON CONFLICT DO NOTHING",&[&key,&actor,&task.project_id,&id]).map_err(TaskWorkflowStore::database_error)?;
            }
        }
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(
            json!({"channel":channel,"audience":audience,"status":"queued_or_skipped_by_recipient_preferences","event_key":key}),
        )
    }
}
