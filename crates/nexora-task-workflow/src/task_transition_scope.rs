use crate::TaskWorkflowStore;
use nexora_platform::{AuthContext, AuthMethod, Capability, PlatformError, WorkTask};
use postgres::Transaction;
use serde_json::Value;

/// Live task scope reads required before execution or failure disclosure.
pub(crate) struct TaskTransitionScope;

impl TaskTransitionScope {
    pub(crate) fn task(
        store: &mut TaskWorkflowStore,
        context: &AuthContext,
        task_id: &str,
        capability: Capability,
    ) -> Result<WorkTask, PlatformError> {
        let mut transaction = store
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        let task = Self::task_in_transaction(&mut transaction, context, task_id, capability)?;
        transaction.commit().map_err(|_| PlatformError::Storage)?;
        Ok(task)
    }

    pub(crate) fn task_in_transaction(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        task_id: &str,
        capability: Capability,
    ) -> Result<WorkTask, PlatformError> {
        let value: Value = transaction
            .query_opt("SELECT to_jsonb(t) FROM tasks t WHERE id=$1", &[&task_id])
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let task: WorkTask = serde_json::from_value(value).map_err(|_| PlatformError::Storage)?;
        Self::require(transaction, context, capability, task.project_id.as_deref())?;
        if let Some(project_id) = task.project_id.as_deref() {
            Self::require_task_relation(transaction, context, project_id, task_id)?;
        }
        Ok(task)
    }

    fn require(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        capability: Capability,
        project_id: Option<&str>,
    ) -> Result<(), PlatformError> {
        if let Some(project_id) = project_id {
            Self::require_live_capability(transaction, context, capability, Some(project_id))?;
            transaction
                .query_opt(
                    "SELECT id FROM projects WHERE id=$1 AND NOT archived FOR SHARE",
                    &[&project_id],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::NotFound)?;
            return Ok(());
        }
        match Self::require_live_capability(transaction, context, capability, None) {
            Ok(()) => Ok(()),
            Err(PlatformError::Forbidden)
                if context.method() != AuthMethod::ApiKey
                    && matches!(
                        capability,
                        Capability::TasksRead
                            | Capability::TasksWrite
                            | Capability::TasksTransition
                    ) =>
            {
                transaction
                    .query_opt(
                        "SELECT role FROM actor_roles WHERE actor_id=$1 AND role IN ('super_admin','developer','technical_lead','pmo','manager') FOR SHARE",
                        &[&context.actor_id()],
                    )
                    .map_err(|_| PlatformError::Storage)?
                    .ok_or(PlatformError::Forbidden)?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn require_live_capability(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        capability: Capability,
        project_id: Option<&str>,
    ) -> Result<(), PlatformError> {
        let actor_kind: String = transaction
            .query_opt(
                "SELECT kind FROM actors WHERE id=$1 AND active FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Unauthorized)?
            .get(0);
        if context.method() == AuthMethod::ApiKey {
            let scope_value: Value = transaction
                .query_opt(
                    "SELECT scope FROM api_keys WHERE id=$1 AND actor_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",
                    &[&context.credential_id(), &context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?
                .get(0);
            let scope: nexora_platform::KeyScope =
                serde_json::from_value(scope_value).map_err(|_| PlatformError::Storage)?;
            scope.validate()?;
            if !scope.allows(capability, project_id) {
                return Err(PlatformError::Forbidden);
            }
        } else {
            if actor_kind != "human" {
                return Err(PlatformError::Unauthorized);
            }
            transaction
                .query_opt(
                    "SELECT s.id FROM sessions s JOIN users u ON u.actor_id=s.actor_id WHERE s.id=$1 AND s.actor_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND s.absolute_expires_at>clock_timestamp() FOR SHARE OF s,u",
                    &[&context.credential_id(), &context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?;
        }
        if !matches!(capability, Capability::RequestsRead | Capability::RequestsWrite)
            && transaction
                .query_opt(
                    "SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('user','supervisor') FOR SHARE",
                    &[&context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .is_some()
        {
            return Err(PlatformError::Forbidden);
        }
        transaction
            .query_opt(
                "SELECT capability FROM actor_grants WHERE actor_id=$1 AND capability=$2 AND (project_id IS NULL OR project_id=$3) FOR SHARE",
                &[&context.actor_id(), &capability.as_str(), &project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Forbidden)?;
        Ok(())
    }

    fn require_task_relation(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        project_id: &str,
        task_id: &str,
    ) -> Result<(), PlatformError> {
        let unrestricted = Self::unrestricted(transaction, context, project_id)?;
        let readable = transaction
            .query_opt(
                "SELECT t.id FROM tasks t WHERE t.project_id=$1 AND t.id=$2 AND ($4 OR t.assignee_id=$3 OR t.created_by=$3 OR (t.assignee_id IS NULL AND (t.lifecycle='pending' OR (t.lifecycle='active' AND EXISTS(SELECT 1 FROM developer_workflows member_workflow WHERE member_workflow.actor_id=$3 AND member_workflow.workflow_id=t.workflow_id)))) OR EXISTS(SELECT 1 FROM task_history h WHERE h.task_id=t.id AND h.project_id=t.project_id AND h.actor_id=$3 AND h.operation IN ('task.created','task.changed','task.reassigned_workflow') AND h.details->'after'->>'assignee_id' IS NOT NULL AND (h.details->'before'->>'assignee_id') IS DISTINCT FROM (h.details->'after'->>'assignee_id')))",
                &[&project_id, &task_id, &context.actor_id(), &unrestricted],
            )
            .map_err(|_| PlatformError::Storage)?;
        if readable.is_none() {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn unrestricted(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        project_id: &str,
    ) -> Result<bool, PlatformError> {
        for (capability, scope) in [
            (Capability::InstallationManage, None),
            (Capability::ActorsManage, None),
            (Capability::ProjectsWrite, Some(project_id)),
        ] {
            match Self::require_live_capability(transaction, context, capability, scope) {
                Ok(()) => return Ok(true),
                Err(PlatformError::Forbidden) => {}
                Err(error) => return Err(error),
            }
        }
        match Self::require_live_capability(
            transaction,
            context,
            Capability::TasksRead,
            Some(project_id),
        ) {
            Ok(()) => {}
            Err(PlatformError::Forbidden) => return Ok(false),
            Err(error) => return Err(error),
        }
        let role = transaction
            .query_opt(
                "SELECT all_projects FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','pmo','manager') FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?;
        if let Some(role) = role {
            if role.get::<_, bool>(0) {
                return Ok(true);
            }
            return Ok(transaction
                .query_opt(
                    "SELECT project_id FROM member_role_projects WHERE actor_id=$1 AND project_id=$2 FOR SHARE",
                    &[&context.actor_id(), &project_id],
                )
                .map_err(|_| PlatformError::Storage)?
                .is_some());
        }
        Ok(false)
    }
}
