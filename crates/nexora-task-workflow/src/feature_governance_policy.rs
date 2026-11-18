use crate::platform_authority::PlatformAuthority;
use nexora_platform::{AuthContext, Capability, PlatformError};
use postgres::Transaction;

pub(crate) struct FeatureGovernancePolicy;

impl FeatureGovernancePolicy {
    pub(crate) fn read(
        transaction: &mut Transaction<'_>,
        context: &AuthContext,
        project_id: &str,
    ) -> Result<(), PlatformError> {
        PlatformAuthority::require(
            transaction,
            context,
            Capability::DocumentsRead,
            Some(project_id),
        )?;
        PlatformAuthority::require(
            transaction,
            context,
            Capability::TasksRead,
            Some(project_id),
        )?;
        PlatformAuthority::project(transaction, project_id)
    }
}
