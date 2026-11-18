use super::TaskTransitionEngine;
use crate::{TaskWorkflowStore, task_transition_run_store::TaskTransitionRunStore};
use nexora_platform::*;
use serde_json::{Value, json};

impl TaskTransitionEngine {
    pub(crate) fn request_approval(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        transition: &str,
        input: &TaskTransitionRequest,
        run: &mut Value,
    ) -> Result<(), PlatformError> {
        if run["approval_request_id"].is_string() {
            return Ok(());
        }
        if let Some(project) = project {
            let mut parameters = input.clone();
            parameters.approval_id = None;
            let request = ApprovalRequestCreate {
                target: ApprovalRequestTarget::Task {
                    task_id: id.into(),
                    transition_id: transition.into(),
                    parameters,
                },
                reason: input.reason.clone().unwrap_or_else(|| {
                    format!("Approval required for task transition {transition}")
                }),
                expires_in_seconds: 86400,
            };
            let key = format!(
                "{}:approval",
                run["id"].as_str().ok_or(PlatformError::Storage)?
            );
            let outer = store.request_context.take();
            let created=store.request(ctx,"approval_request.create",&key,&json!({"project_id":project,"request":request}),|store|crate::approval_request_definition_store::ApprovalRequestDefinitionStore::create(store,ctx,project,&request));
            store.request_context = outer;
            let request = created?;
            run["approval_request_id"] = json!(request.id);
            TaskTransitionRunStore::save(store, run)?;
        } else {
            let key = format!(
                "{}:approval",
                run["id"].as_str().ok_or(PlatformError::Storage)?
            );
            store.client().execute("INSERT INTO in_app_notifications(id,recipient_id,kind,resource_id) SELECT $1||':'||u.actor_id,u.actor_id,'task_updated',$2 FROM users u JOIN actors a ON a.id=u.actor_id AND a.active WHERE u.actor_id<>$3 AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=u.actor_id AND g.project_id IS NULL AND g.capability='approvals:grant') ON CONFLICT DO NOTHING",&[&key,&id,&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?;
        }
        Ok(())
    }
}
