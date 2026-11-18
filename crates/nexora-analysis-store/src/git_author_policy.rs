use crate::AnalysisStore;
use nexora_platform::PlatformError;
use postgres::Transaction;
pub(crate) struct GitAuthorPolicy;
impl GitAuthorPolicy {
    pub fn lock(tx: &mut Transaction<'_>) -> Result<(), PlatformError> {
        tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended(current_schema()||':git-author-aliases',0))",&[]).map_err(AnalysisStore::database_error)?;
        Ok(())
    }
}
