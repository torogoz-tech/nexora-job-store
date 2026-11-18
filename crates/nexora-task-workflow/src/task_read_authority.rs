use crate::{
    TaskWorkflowStore, platform_authority::PlatformAuthority,
    task_read_predicate::task_read_predicate,
};
use nexora_platform::{AuthContext, Capability, PlatformError};
use postgres::Transaction;
pub(crate) struct TaskReadAuthority;
impl TaskReadAuthority {
    pub fn unrestricted(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
    ) -> Result<bool, PlatformError> {
        for (capability, scope) in [
            (Capability::InstallationManage, None),
            (Capability::ActorsManage, None),
            (Capability::ProjectsWrite, Some(project)),
        ] {
            match PlatformAuthority::require(tx, ctx, capability, scope) {
                Ok(()) => return Ok(true),
                Err(PlatformError::Forbidden) => {}
                Err(error) => return Err(error),
            }
        }
        // Coordination roles can follow all work only inside their current project scope.
        // Keep API-key attenuation: the live TasksRead scope must also allow this project.
        match PlatformAuthority::require(tx, ctx, Capability::TasksRead, Some(project)) {
            Ok(()) => {}
            Err(PlatformError::Forbidden) => return Ok(false),
            Err(error) => return Err(error),
        }
        let role = tx.query_opt("SELECT all_projects FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','pmo','manager') FOR SHARE", &[&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?;
        if let Some(role) = role {
            if role.get::<_, bool>(0) {
                return Ok(true);
            }
            return Ok(tx.query_opt("SELECT project_id FROM member_role_projects WHERE actor_id=$1 AND project_id=$2 FOR SHARE", &[&ctx.actor_id(), &project]).map_err(TaskWorkflowStore::database_error)?.is_some());
        }
        Ok(false)
    }
    pub fn require_relation(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        id: &str,
    ) -> Result<(), PlatformError> {
        let unrestricted = Self::unrestricted(tx, ctx, project)?;
        let predicate = task_read_predicate("$3", "$4");
        tx.query_opt(
            &format!("SELECT t.id FROM tasks t WHERE t.project_id=$1 AND t.id=$2 AND {predicate}"),
            &[&project, &id, &ctx.actor_id(), &unrestricted],
        )
        .map_err(TaskWorkflowStore::database_error)?
        .ok_or(PlatformError::NotFound)?;
        Ok(())
    }
}
