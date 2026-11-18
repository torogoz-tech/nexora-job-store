use crate::TaskWorkflowStore;
use nexora_platform::{AuthContext, AuthMethod, Capability, PlatformError};
use serde_json::Value;

/// Reads live repository/provider authority facts using the existing API-owned schema.
pub(crate) struct TaskTransitionProviderAuthority;

impl TaskTransitionProviderAuthority {
    pub(crate) fn read(
        store: &mut TaskWorkflowStore,
        context: &AuthContext,
        project_id: &str,
        repository_id: &str,
    ) -> Result<Value, PlatformError> {
        context.require(Capability::RepositoriesRead, Some(project_id))?;
        let mut transaction = store
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        transaction
            .query_opt(
                "SELECT id FROM projects WHERE id=$1 AND NOT archived FOR SHARE",
                &[&project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?;
        let actor_kind: String = transaction
            .query_opt(
                "SELECT kind FROM actors WHERE id=$1 AND active FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Unauthorized)?
            .get(0);
        if context.method() == AuthMethod::Session && actor_kind != "human" {
            return Err(PlatformError::Unauthorized);
        }
        if context.method() == AuthMethod::ApiKey {
            let scope: Value = transaction
                .query_opt(
                    "SELECT scope FROM api_keys WHERE id=$1 AND actor_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",
                    &[&context.credential_id(), &context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?
                .get(0);
            let scope: nexora_platform::KeyScope =
                serde_json::from_value(scope).map_err(|_| PlatformError::Storage)?;
            scope.validate()?;
            if !scope.allows(Capability::RepositoriesRead, Some(project_id)) {
                return Err(PlatformError::Forbidden);
            }
        } else {
            transaction
                .query_opt(
                    "SELECT s.id FROM sessions s JOIN users u ON u.actor_id=s.actor_id WHERE s.id=$1 AND s.actor_id=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND s.absolute_expires_at>clock_timestamp() FOR SHARE OF s,u",
                    &[&context.credential_id(), &context.actor_id()],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?;
        }
        let grant = transaction
            .query_opt(
                "SELECT 1 FROM actor_grants WHERE actor_id=$1 AND capability='repositories:read' AND (project_id IS NULL OR project_id=$2) FOR SHARE",
                &[&context.actor_id(), &project_id],
            )
            .map_err(|_| PlatformError::Storage)?;
        if grant.is_none() {
            return Err(PlatformError::Forbidden);
        }
        let restricted_role = transaction
            .query_opt(
                "SELECT role FROM actor_roles WHERE actor_id=$1 AND role IN ('user','supervisor') FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?;
        if restricted_role.is_some() {
            return Err(PlatformError::Forbidden);
        }
        let binding = transaction
            .query_opt(
                "SELECT connection_id,source_kind,revision FROM project_repositories WHERE id=$1 AND project_id=$2 AND NOT archived FOR SHARE",
                &[&repository_id, &project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?;
        if binding.get::<_, String>(1) != "git" {
            return Err(PlatformError::Validation(
                "local snapshots do not support Git jobs",
            ));
        }
        let connection_id: String =
            binding
                .get::<_, Option<String>>(0)
                .ok_or(PlatformError::Conflict(
                    "repository source is not authorized",
                ))?;
        let connection = transaction
            .query_opt(
                "SELECT owner_actor_id,revision FROM git_connections WHERE id=$1 AND status='active' FOR SHARE",
                &[&connection_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Conflict("repository source is not authorized"))?;
        let owner: String = connection.get(0);
        transaction
            .query_opt(
                "SELECT id FROM actors WHERE id=$1 AND active FOR SHARE",
                &[&owner],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Conflict(
                "repository source owner is inactive",
            ))?;
        if owner != context.actor_id() {
            let admin = Self::has_installation_manage(&mut transaction, context)?;
            if !admin {
                transaction
                    .query_opt(
                        "SELECT connection_id FROM git_connection_projects WHERE connection_id=$1 AND project_id=$2 FOR SHARE",
                        &[&connection_id, &project_id],
                    )
                    .map_err(|_| PlatformError::Storage)?
                    .ok_or(PlatformError::Forbidden)?;
            }
        }
        let metadata: Value = transaction
            .query_opt(
                "SELECT repository_metadata(r) FROM project_repositories r WHERE id=$1 AND project_id=$2 AND connection_id=$3 AND NOT archived AND authorization_status='authorized' FOR SHARE",
                &[&repository_id, &project_id, &connection_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let revision = metadata["revision"]
            .as_i64()
            .ok_or(PlatformError::Storage)?;
        let remote_id = metadata["remote_id"].clone();
        let remote_full_name = metadata["remote_full_name"].clone();
        let provider = metadata["provider"].clone();
        let host = metadata["host"].clone();
        let repository_revision: i64 = binding.get(2);
        let connection_revision: i64 = connection.get(1);
        let credential_revision: i64 = transaction
            .query_one(
                "SELECT credential_revision FROM git_connections WHERE id=$1",
                &[&connection_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .get(0);
        let source_generation: i64 = transaction
            .query_one(
                "SELECT source_generation FROM project_repositories WHERE id=$1 AND project_id=$2",
                &[&repository_id, &project_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .get(0);
        if revision != repository_revision {
            return Err(PlatformError::Conflict("repository revision changed"));
        }
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                &[&format!("development-policy:{project_id}:{repository_id}")],
            )
            .map_err(|_| PlatformError::Storage)?;
        let policy_revision = transaction
            .query_opt(
                "SELECT revision FROM repository_development_policies WHERE project_id=$1 AND repository_id=$2 FOR SHARE",
                &[&project_id, &repository_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .map(|row| row.get::<_, i64>(0))
            .unwrap_or(0);
        let facts = serde_json::json!({
            "repository_id": repository_id,
            "repository_revision": revision,
            "remote_id": remote_id,
            "remote_full_name": remote_full_name,
            "provider": provider,
            "host": host,
            "source_generation": source_generation,
            "connection_id": connection_id,
            "connection_revision": connection_revision,
            "credential_revision": credential_revision,
            "policy_revision": policy_revision,
        });
        transaction.commit().map_err(|_| PlatformError::Storage)?;
        Ok(facts)
    }

    fn has_installation_manage(
        transaction: &mut postgres::Transaction<'_>,
        context: &AuthContext,
    ) -> Result<bool, PlatformError> {
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
            if !scope.allows(Capability::InstallationManage, None) {
                return Ok(false);
            }
        }
        let role_restricted = transaction
            .query_opt(
                "SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('user','supervisor') FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .is_some();
        if role_restricted {
            return Ok(false);
        }
        Ok(transaction
            .query_opt(
                "SELECT capability FROM actor_grants WHERE actor_id=$1 AND capability='installation:manage' AND project_id IS NULL FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .is_some())
    }
}
