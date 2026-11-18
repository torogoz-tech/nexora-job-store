use crate::{TaskWorkflowStore, platform_authority::PlatformAuthority};
use nexora_platform::{AuthContext, Capability, PlatformError, RepositorySourceKind};
use postgres::{Transaction, types::Type};
use serde_json::Value;

/// Common prefix of Git and local source authority; it does not lock the repository early.
pub(crate) struct RepositoryAccess {
    pub revision: i64,
    pub source_kind: RepositorySourceKind,
}
impl RepositoryAccess {
    pub fn lock(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        project: &str,
        repository: &str,
    ) -> Result<Self, PlatformError> {
        for capability in [Capability::RepositoriesRead, Capability::KnowledgeRead] {
            PlatformAuthority::require(tx, ctx, capability, Some(project))
                .map_err(Self::conceal)?;
        }
        PlatformAuthority::project(tx, project)?;
        let row = tx.query_typed_opt("SELECT revision,source_kind FROM project_repositories WHERE id=$1 AND project_id=$2 AND NOT archived", &[(&repository, Type::TEXT),(&project, Type::TEXT)])
            .map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?;
        Ok(Self {
            revision: row.get(0),
            source_kind: TaskWorkflowStore::decode(Value::String(row.get(1)))?,
        })
    }
    pub fn conceal(error: PlatformError) -> PlatformError {
        match error {
            PlatformError::Forbidden | PlatformError::Conflict(_) => PlatformError::NotFound,
            other => other,
        }
    }
}
