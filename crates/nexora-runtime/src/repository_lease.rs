/// Closing this dedicated connection releases its PostgreSQL session advisory lock on every path.
pub(crate) struct RepositoryLease {
    pub _connection: postgres::Client,
}

impl RepositoryLease {
    pub fn lock_transaction(
        tx: &mut postgres::Transaction<'_>,
        tenant: &nexora_core::TenantId,
        repository: &nexora_core::RepositoryId,
    ) -> Result<(), nexora_core::NexoraError> {
        let error =
            || nexora_core::NexoraError::Storage("knowledge repository lock unavailable".into());
        let key = serde_json::to_string(&(&tenant.0, &repository.0)).map_err(|_| error())?;
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            &[&key],
        )
        .map_err(|_| error())?;
        Ok(())
    }
}
