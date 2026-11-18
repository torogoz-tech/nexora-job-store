use crate::{AnalysisStore, platform_authority::PlatformAuthority};
use nexora_platform::{AuthContext, Capability, PlatformError};
use postgres::Transaction;

pub(crate) struct RepositoryJobSource {
    pub connection: String,
    pub connection_revision: i64,
    pub generation: i64,
}
impl RepositoryJobSource {
    pub fn lock(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
        expected_revision: i64,
    ) -> Result<Self, PlatformError> {
        Self::lock_source(tx, ctx, project, repository, expected_revision, true)
    }
    /// Published evidence follows project read authority, without delegating Git credentials.
    fn lock_source(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
        expected_revision: i64,
        require_delegation: bool,
    ) -> Result<Self, PlatformError> {
        // Match provider-binding lock order: project, connection, repository.
        let row = tx.query_opt("SELECT connection_id,source_kind FROM project_repositories WHERE id=$1 AND project_id=$2 AND NOT archived", &[&repository,&project]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::NotFound)?;
        if row.get::<_, String>(1) != "git" {
            return Err(PlatformError::Validation(
                "local snapshots do not support Git jobs",
            ));
        }
        let connection: String = row
            .get::<_, Option<String>>(0)
            .ok_or(PlatformError::Conflict(
                "repository source is not authorized",
            ))?;
        let row = tx.query_opt("SELECT owner_actor_id,revision FROM git_connections WHERE id=$1 AND status='active' FOR SHARE", &[&connection]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("repository source is not authorized"))?;
        let owner: String = row.get(0);
        let connection_revision: i64 = row.get(1);
        tx.query_opt(
            "SELECT id FROM actors WHERE id=$1 AND active FOR SHARE",
            &[&owner],
        )
        .map_err(AnalysisStore::database_error)?
        .ok_or(PlatformError::Conflict(
            "repository source owner is inactive",
        ))?;
        if require_delegation && owner != ctx.actor_id() {
            let admin =
                match PlatformAuthority::require(tx, ctx, Capability::InstallationManage, None) {
                    Ok(()) => true,
                    Err(PlatformError::Forbidden) => false,
                    Err(error) => return Err(error),
                };
            if !admin {
                tx.query_opt("SELECT connection_id FROM git_connection_projects WHERE connection_id=$1 AND project_id=$2 FOR SHARE", &[&connection,&project]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Forbidden)?;
            }
        }
        let row = tx.query_opt("SELECT revision,source_generation,authorization_status FROM project_repositories WHERE id=$1 AND project_id=$2 AND connection_id=$3 AND NOT archived FOR SHARE", &[&repository,&project,&connection]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Conflict("repository changed during enqueue"))?;
        let revision: i64 = row.get(0);
        let generation: i64 = row.get(1);
        if row.get::<_, String>(2) != "authorized" {
            return Err(PlatformError::Conflict(
                "repository source is not authorized",
            ));
        }
        if revision != expected_revision {
            return Err(PlatformError::Conflict("repository revision changed"));
        }
        Ok(Self {
            connection,
            connection_revision,
            generation,
        })
    }
}
