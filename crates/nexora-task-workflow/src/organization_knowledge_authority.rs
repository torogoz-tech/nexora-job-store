use crate::platform_authority::PlatformAuthority;
use nexora_platform::{AuthContext, Capability, PlatformError};
use postgres::Transaction;
pub(crate) struct OrganizationKnowledgeAuthority;
impl OrganizationKnowledgeAuthority {
    pub fn read(tx: &mut Transaction<'_>, ctx: &AuthContext) -> Result<(), PlatformError> {
        PlatformAuthority::require(tx, ctx, Capability::OrganizationKnowledgeRead, None)
    }
}
