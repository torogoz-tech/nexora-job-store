use crate::KnowledgeStore;
use nexora_platform::PlatformError;

impl KnowledgeStore {
    pub fn refresh_developer_ratings(&mut self) -> Result<u64, PlatformError> {
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        transaction
            .query(
                "SELECT singleton FROM labor_calendar_settings FOR UPDATE",
                &[],
            )
            .map_err(Self::database_error)?;
        let dirty = transaction
            .query(
                "SELECT actor_id FROM developer_rating_dirty FOR UPDATE",
                &[],
            )
            .map_err(Self::database_error)?
            .into_iter()
            .map(|row| row.get::<_, String>(0))
            .collect::<Vec<_>>();
        let changed = transaction
            .query_one(include_str!("developer_rating_refresh.sql"), &[])
            .map_err(Self::database_error)?
            .get::<_, i64>(0) as u64;
        transaction
            .execute(
                "INSERT INTO developer_rating_closures(actor_id,last_closed,calendar_revision) SELECT r.actor_id,(transaction_timestamp() AT TIME ZONE s.timezone)::date-1,s.revision FROM actor_roles r CROSS JOIN labor_calendar_settings s WHERE r.role IN ('developer','technical_lead') ON CONFLICT(actor_id) DO UPDATE SET last_closed=EXCLUDED.last_closed,calendar_revision=EXCLUDED.calendar_revision",
                &[],
            )
            .map_err(Self::database_error)?;
        transaction
            .execute(
                "DELETE FROM developer_rating_dirty WHERE actor_id=ANY($1)",
                &[&dirty],
            )
            .map_err(Self::database_error)?;
        transaction.commit().map_err(Self::database_error)?;
        Ok(changed)
    }
}
