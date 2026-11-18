use nexora_database_connections::PostgresConnection;
use nexora_platform::{
    AuthContext, AuthMethod, Capability, GitConnection, PlatformError, ProjectRepository,
    RepositoryAuthorization, RepositorySourceKind,
};
use postgres::Transaction;
use postgres::types::Type;
use serde_json::{Value, json};

pub(crate) struct ReleaseAuthority;

impl ReleaseAuthority {
    pub(crate) fn authorize_release_tasks(
        client: &mut PostgresConnection,
        context: &AuthContext,
        project: &str,
        release: &str,
    ) -> Result<(), PlatformError> {
        let mut transaction = client
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        let tasks = transaction
            .query(
                "SELECT i.task_id FROM task_release_items i WHERE i.release_id=$1 ORDER BY i.task_id",
                &[&release],
            )
            .map_err(|_| PlatformError::Storage)?;

        for task_row in tasks {
            let task_id: String = task_row.get(0);
            transaction
                .query_opt(
                    "SELECT t.id FROM tasks t WHERE t.id=$1 AND t.project_id=$2",
                    &[&task_id, &project],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::NotFound)?;
            Self::require_in_transaction(
                &mut transaction,
                context,
                Capability::TasksRead,
                Some(project),
            )
            .map_err(|error| {
                if matches!(error, PlatformError::Forbidden) {
                    PlatformError::NotFound
                } else {
                    error
                }
            })?;

            if !Self::release_task_is_unrestricted(&mut transaction, context, project)? {
                let actor = context.actor_id();
                let unrestricted = false;
                let predicate = "(t.assignee_id=$3 OR t.created_by=$3 OR (t.assignee_id IS NULL AND (t.lifecycle='pending' OR (t.lifecycle='active' AND EXISTS(SELECT 1 FROM developer_workflows w WHERE w.actor_id=$3 AND w.workflow_id=t.workflow_id)))) OR EXISTS(SELECT 1 FROM task_history h WHERE h.task_id=t.id AND h.project_id=t.project_id AND h.actor_id=$3 AND h.operation IN ('task.created','task.changed','task.reassigned_workflow') AND h.details->'after'->>'assignee_id' IS NOT NULL AND (h.details->'before'->>'assignee_id') IS DISTINCT FROM (h.details->'after'->>'assignee_id')))";
                let query = format!(
                    "SELECT t.id FROM tasks t WHERE t.id=$1 AND t.project_id=$2 AND ({unrestricted} OR {predicate})"
                );
                transaction
                    .query_opt(&query, &[&task_id, &project, &actor])
                    .map_err(|_| PlatformError::Storage)?
                    .ok_or(PlatformError::NotFound)?;
            }
        }

        transaction.commit().map_err(|_| PlatformError::Storage)
    }

    fn release_task_is_unrestricted(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        project: &str,
    ) -> Result<bool, PlatformError> {
        for (capability, scope) in [
            (Capability::InstallationManage, None),
            (Capability::ActorsManage, None),
            (Capability::ProjectsWrite, Some(project)),
        ] {
            match Self::require_in_transaction(transaction, context, capability, scope) {
                Ok(()) => return Ok(true),
                Err(PlatformError::Forbidden) => {}
                Err(error) => return Err(error),
            }
        }

        let role = transaction
            .query_opt(
                "SELECT all_projects FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','pmo','manager') FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?;
        let Some(role) = role else {
            return Ok(false);
        };
        if role.get::<_, bool>(0) {
            return Ok(true);
        }

        Ok(transaction
            .query_opt(
                "SELECT project_id FROM member_role_projects WHERE actor_id=$1 AND project_id=$2 FOR SHARE",
                &[&context.actor_id(), &project],
            )
            .map_err(|_| PlatformError::Storage)?
            .is_some())
    }

    pub(crate) fn require_context(
        context: &AuthContext,
        capability: Capability,
        project: &str,
    ) -> Result<(), PlatformError> {
        context.require(capability, Some(project))
    }

    pub(crate) fn require_installation_context(
        context: &AuthContext,
        capability: Capability,
    ) -> Result<(), PlatformError> {
        context.require(capability, None)
    }

    fn require_in_transaction(
        client: &mut Transaction<'_>,
        context: &AuthContext,
        capability: Capability,
        project: Option<&str>,
    ) -> Result<(), PlatformError> {
        context.require(capability, project)?;
        let actor = context.actor_id();
        let credential = context.credential_id();
        let kind: String = client
            .query_typed_opt(
                "SELECT kind FROM actors WHERE id=$1 AND active FOR SHARE",
                &[(&actor, Type::TEXT)],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Unauthorized)?
            .get(0);

        if context.method() == AuthMethod::ApiKey {
            let scope: serde_json::Value = client
                .query_typed_opt(
                    "SELECT scope FROM api_keys WHERE id=$1 AND actor_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",
                    &[(&credential, Type::TEXT), (&actor, Type::TEXT)],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?
                .get(0);
            let scope: nexora_platform::KeyScope =
                serde_json::from_value(scope).map_err(|_| PlatformError::Storage)?;
            scope.validate()?;
            if !scope.allows(capability, project) {
                return Err(PlatformError::Forbidden);
            }
        } else {
            if kind != "human" {
                return Err(PlatformError::Unauthorized);
            }
            client
                .query_typed_opt(
                    "SELECT s.id FROM sessions s JOIN users u ON u.actor_id=s.actor_id WHERE s.id=$1 AND s.actor_id=$2 AND s.access_stage='ready' AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND s.absolute_expires_at>clock_timestamp() FOR SHARE OF s,u",
                    &[(&credential, Type::TEXT), (&actor, Type::TEXT)],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::Unauthorized)?;
        }

        if !matches!(capability, Capability::RequestsRead | Capability::RequestsWrite)
            && client
                .query_opt(
                    "SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('user','supervisor') FOR SHARE",
                    &[&actor],
                )
                .map_err(|_| PlatformError::Storage)?
                .is_some()
        {
            return Err(PlatformError::Forbidden);
        }

        client
            .query_typed_opt(
                "SELECT capability FROM actor_grants WHERE actor_id=$1 AND capability=$2 AND (project_id IS NULL OR $3::text IS NOT NULL AND project_id=$3) FOR SHARE",
                &[
                    (&actor, Type::TEXT),
                    (&capability.as_str(), Type::TEXT),
                    (&project, Type::TEXT),
                ],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Forbidden)?;
        if let Some(project) = project {
            client
                .query_typed_opt(
                    "SELECT id FROM projects WHERE id=$1 AND NOT archived FOR SHARE",
                    &[(&project, Type::TEXT)],
                )
                .map_err(|_| PlatformError::Storage)?
                .ok_or(PlatformError::NotFound)?;
        }

        Ok(())
    }

    pub(crate) fn authorize_publication(
        client: &mut PostgresConnection,
        context: &AuthContext,
        project: &str,
    ) -> Result<(), PlatformError> {
        Self::require_context(context, Capability::TasksWrite, project)?;
        let mut transaction = client
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        Self::require_in_transaction(
            &mut transaction,
            context,
            Capability::TasksWrite,
            Some(project),
        )?;
        transaction
            .query_opt(
                "SELECT actor_id FROM actor_roles WHERE actor_id=$1 AND role IN ('technical_lead','manager','super_admin') FOR SHARE",
                &[&context.actor_id()],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Forbidden)?;
        transaction.commit().map_err(|_| PlatformError::Storage)
    }

    pub(crate) fn lock_repository_source(
        client: &mut PostgresConnection,
        context: &AuthContext,
        project: &str,
        repository_id: &str,
    ) -> Result<(GitConnection, ProjectRepository, Value), PlatformError> {
        let mut transaction = client
            .client()
            .transaction()
            .map_err(|_| PlatformError::Storage)?;
        Self::require_in_transaction(
            &mut transaction,
            context,
            Capability::RepositoriesRead,
            Some(project),
        )?;

        let metadata: Value = transaction
            .query_opt(
                "SELECT repository_metadata(r) FROM project_repositories r WHERE id=$1 AND project_id=$2 AND NOT archived FOR SHARE OF r",
                &[&repository_id, &project],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::NotFound)?
            .get(0);
        let repository: ProjectRepository =
            serde_json::from_value(metadata).map_err(|_| PlatformError::Storage)?;
        if repository.source_kind != RepositorySourceKind::Git
            || repository.archived
            || repository.authorization_status != RepositoryAuthorization::Authorized
        {
            return Err(PlatformError::Conflict(
                "repository source is not authorized",
            ));
        }
        let connection_id = repository
            .connection_id
            .as_deref()
            .ok_or(PlatformError::Conflict(
                "repository source is not authorized",
            ))?;
        let connection_row = transaction
            .query_opt(
                "SELECT to_jsonb(c)-'tokens',c.credential_revision FROM git_connections c JOIN actors a ON a.id=c.owner_actor_id WHERE c.id=$1 AND c.status='active' AND a.active FOR SHARE OF c,a",
                &[&connection_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Conflict(
                "repository source is not authorized",
            ))?;
        let connection: GitConnection =
            serde_json::from_value(connection_row.get(0)).map_err(|_| PlatformError::Storage)?;
        let credential_revision: i64 = connection_row.get(1);

        if connection.owner_actor_id != context.actor_id() {
            match Self::require_installation_context(context, Capability::InstallationManage) {
                Ok(()) => Self::require_in_transaction(
                    &mut transaction,
                    context,
                    Capability::InstallationManage,
                    None,
                )?,
                Err(PlatformError::Forbidden) => {
                    transaction
                        .query_opt(
                            "SELECT connection_id FROM git_connection_projects WHERE connection_id=$1 AND project_id=$2 FOR SHARE",
                            &[&connection_id, &project],
                        )
                        .map_err(|_| PlatformError::Storage)?
                        .ok_or(PlatformError::Forbidden)?;
                }
                Err(error) => return Err(error),
            }
        }

        let policy_lock = format!("development-policy:{project}:{repository_id}");
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                &[&policy_lock],
            )
            .map_err(|_| PlatformError::Storage)?;
        let policy_revision = transaction
            .query_opt(
                "SELECT revision FROM repository_development_policies WHERE project_id=$1 AND repository_id=$2 FOR SHARE",
                &[&project, &repository_id],
            )
            .map_err(|_| PlatformError::Storage)?
            .map(|row| row.get::<_, i64>(0))
            .unwrap_or_default();

        let fact = json!({
            "repository_id": repository_id,
            "repository_revision": repository.revision,
            "remote_id": repository.remote_id,
            "remote_full_name": repository.remote_full_name,
            "provider": repository.provider,
            "host": repository.host,
            "source_generation": repository.source_generation,
            "connection_id": connection.id,
            "connection_revision": connection.revision,
            "credential_revision": credential_revision,
            "policy_revision": policy_revision
        });
        transaction.commit().map_err(|_| PlatformError::Storage)?;

        Ok((connection, repository, fact))
    }

    pub(crate) fn validate_release_pins(
        client: &mut PostgresConnection,
        project: &str,
        release: &str,
    ) -> Result<(), PlatformError> {
        let invalid: bool = client
            .client()
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM task_release_items i LEFT JOIN tasks t ON t.id=i.task_id WHERE i.release_id=$1 AND (t.id IS NULL OR t.project_id IS DISTINCT FROM $2 OR t.revision<>i.task_revision OR t.archived OR t.lifecycle<>'active')) OR EXISTS(SELECT 1 FROM task_release_commit_pins p LEFT JOIN task_commit_links l ON l.task_id=p.task_id AND l.repository_id=p.repository_id AND l.commit_sha=p.commit_sha AND l.removed_at IS NULL WHERE p.release_id=$1 AND (l.id IS NULL OR l.provider_evidence->>'provenance' IS DISTINCT FROM 'provider_verified'))",
                &[&release, &project],
            )
            .map_err(|_| PlatformError::Storage)?
            .get(0);
        if invalid {
            return Err(PlatformError::Conflict(
                "selected release task revisions or commit bindings changed",
            ));
        }
        Ok(())
    }
}
