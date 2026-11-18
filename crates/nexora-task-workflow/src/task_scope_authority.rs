use crate::{
    TaskWorkflowStore, platform_authority::PlatformAuthority,
    task_read_authority::TaskReadAuthority,
};
use nexora_platform::*;
use postgres::Transaction;

pub(crate) struct TaskScopeAuthority;
impl TaskScopeAuthority {
    pub fn require(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        capability: Capability,
        project: Option<&str>,
    ) -> Result<(), PlatformError> {
        if let Some(project) = project {
            PlatformAuthority::require(tx, ctx, capability, Some(project))?;
            return PlatformAuthority::project(tx, project);
        }
        // Always validate the live identity/credential before allowing a human role exception.
        match PlatformAuthority::require(tx, ctx, capability, None) {
            Ok(()) => Ok(()),
            Err(PlatformError::Forbidden)
                if ctx.method() != AuthMethod::ApiKey
                    && matches!(
                        capability,
                        Capability::TasksRead
                            | Capability::TasksWrite
                            | Capability::TasksTransition
                    ) =>
            {
                tx.query_opt("SELECT role FROM actor_roles WHERE actor_id=$1 AND role IN ('super_admin','developer','technical_lead','pmo','manager') FOR SHARE", &[&ctx.actor_id()])
                    .map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
                Ok(())
            }
            Err(PlatformError::Forbidden)
                if ctx.method() == AuthMethod::Session
                    && capability == Capability::ApprovalsGrant =>
            {
                tx.query_opt("SELECT role FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','pmo','manager','super_admin') FOR SHARE",&[&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    pub fn relation(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
    ) -> Result<(), PlatformError> {
        if let Some(project) = project {
            TaskReadAuthority::require_relation(tx, ctx, project, id)?;
        }
        Ok(())
    }
    pub fn modify(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: Option<&str>,
        id: &str,
        self_reservation: bool,
    ) -> Result<(), PlatformError> {
        if project.is_some() {
            return Self::relation(tx, ctx, project, id);
        }
        tx.query_opt("SELECT t.id FROM tasks t WHERE t.id=$1 AND t.project_id IS NULL AND (t.created_by=$2 OR t.assignee_id=$2 OR ($3 AND t.assignee_id IS NULL AND t.lifecycle='pending') OR EXISTS(SELECT 1 FROM actor_roles r WHERE r.actor_id=$2 AND r.role IN ('technical_lead','pmo','manager','super_admin'))) FOR UPDATE OF t",&[&id,&ctx.actor_id(),&self_reservation]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Forbidden)?;
        Ok(())
    }
    pub fn task(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        id: &str,
        capability: Capability,
    ) -> Result<WorkTask, PlatformError> {
        let value: serde_json::Value = tx
            .query_opt("SELECT to_jsonb(t) FROM tasks t WHERE id=$1", &[&id])
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let task: WorkTask = TaskWorkflowStore::decode(value)?;
        Self::require(tx, ctx, capability, task.project_id.as_deref()).map_err(|error| {
            if matches!(error, PlatformError::Forbidden) {
                PlatformError::NotFound
            } else {
                error
            }
        })?;
        Self::relation(tx, ctx, task.project_id.as_deref(), id)?;
        Ok(task)
    }
}
