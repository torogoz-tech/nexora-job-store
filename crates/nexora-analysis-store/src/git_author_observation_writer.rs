use crate::AnalysisStore;
use nexora_platform::PlatformError;
use postgres::Transaction;
pub(crate) struct GitAuthorObservationWriter;
impl GitAuthorObservationWriter {
    pub fn attribute(
        tx: &mut Transaction<'_>,
        alias: Option<&str>,
        repository: Option<&str>,
    ) -> Result<(), PlatformError> {
        let members=tx.query("SELECT member.id FROM actors member WHERE EXISTS(SELECT 1 FROM git_author_aliases a JOIN git_commit_authorship c ON c.author_email=a.email AND c.provider=a.provider AND c.host=a.host WHERE a.actor_id=member.id AND a.revoked_at IS NULL AND c.actor_id IS NULL AND ($1::text IS NULL OR a.id=$1) AND ($2::text IS NULL OR c.repository_id=$2)) ORDER BY member.id FOR SHARE OF member",&[&alias,&repository]).map_err(AnalysisStore::database_error)?;
        for member in members {
            let id: String = member.get(0);
            tx.query("SELECT capability FROM actor_grants WHERE actor_id=$1 AND capability='repositories:read' FOR SHARE",&[&id]).map_err(AnalysisStore::database_error)?;
        }
        // Authorship is organization-wide; query authorization separately controls who can read the work.
        let rows=tx.query("UPDATE git_commit_authorship c SET actor_id=a.actor_id,alias_id=a.id,attributed_at=clock_timestamp() FROM git_author_aliases a,actors member WHERE c.actor_id IS NULL AND c.author_email=a.email AND c.provider=a.provider AND c.host=a.host AND a.revoked_at IS NULL AND ($1::text IS NULL OR a.id=$1) AND ($2::text IS NULL OR c.repository_id=$2) AND member.id=a.actor_id AND member.active AND member.kind='human' RETURNING c.actor_id",&[&alias,&repository]).map_err(AnalysisStore::database_error)?;
        for row in rows {
            let actor: String = row.get(0);
            tx.execute(
                "INSERT INTO developer_rating_dirty(actor_id) VALUES($1) ON CONFLICT DO NOTHING",
                &[&actor],
            )
            .map_err(AnalysisStore::database_error)?;
        }
        Ok(())
    }
}
