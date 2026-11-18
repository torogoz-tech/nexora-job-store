use crate::TaskWorkflowStore;
use nexora_platform::{AuthContext, AuthMethod, Capability, KeyScope, PlatformError};
use postgres::{Transaction, types::Type};
use serde_json::Value;

/// Locks current platform authority, including deletable grant rows.
pub(crate) struct PlatformAuthority;
impl PlatformAuthority {
    pub fn require(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        capability: Capability,
        project: Option<&str>,
    ) -> Result<(), PlatformError> {
        let kind: String = tx
            .query_typed_opt(
                "SELECT kind FROM actors WHERE id=$1 AND active FOR SHARE",
                &[(&ctx.actor_id(), Type::TEXT)],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Unauthorized)?
            .get(0);
        if ctx.method() == AuthMethod::ApiKey {
            let row = tx.query_typed_opt("SELECT scope FROM api_keys WHERE id=$1 AND actor_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE", &[(&ctx.credential_id(), Type::TEXT),(&ctx.actor_id(), Type::TEXT)]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
            let scope: KeyScope = TaskWorkflowStore::decode(row.get::<_, Value>(0))?;
            scope.validate()?;
            if !scope.allows(capability, project) {
                return Err(PlatformError::Forbidden);
            }
        } else {
            if kind != "human" {
                return Err(PlatformError::Unauthorized);
            }
            tx.query_typed_opt("SELECT s.id FROM sessions s JOIN users u ON u.actor_id=s.actor_id WHERE s.id=$1 AND s.actor_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() FOR SHARE OF s,u", &[(&ctx.credential_id(), Type::TEXT),(&ctx.actor_id(), Type::TEXT)]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
        }
        if !matches!(capability,Capability::RequestsRead|Capability::RequestsWrite) && tx.query_opt("SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('user','supervisor') FOR SHARE", &[&ctx.actor_id()]).map_err(TaskWorkflowStore::database_error)?.is_some() { return Err(PlatformError::Forbidden); }
        let rows = tx.query_typed("SELECT capability FROM actor_grants WHERE actor_id=$1 AND capability=$2 AND (project_id IS NULL OR project_id=$3) FOR SHARE", &[(&ctx.actor_id(), Type::TEXT),(&capability.as_str(), Type::TEXT),(&project, Type::TEXT)]).map_err(TaskWorkflowStore::database_error)?;
        if rows.is_empty() {
            return Err(PlatformError::Forbidden);
        }
        Ok(())
    }
    pub fn project(tx: &mut Transaction<'_>, project: &str) -> Result<(), PlatformError> {
        tx.query_typed_opt(
            "SELECT id FROM projects WHERE id=$1 AND NOT archived FOR SHARE",
            &[(&project, Type::TEXT)],
        )
        .map_err(TaskWorkflowStore::database_error)?
        .ok_or(PlatformError::NotFound)?;
        Ok(())
    }
}
