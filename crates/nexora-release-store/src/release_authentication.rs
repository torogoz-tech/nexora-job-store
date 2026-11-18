use nexora_database_connections::PostgresConnection;
use nexora_platform::{
    ActorKind, AuthContext, AuthMethod, CapabilityGrant, KeyScope, PlatformError,
};
use serde_json::Value;

pub(crate) struct ReleaseAuthentication;

impl ReleaseAuthentication {
    pub(crate) fn load_context(
        client: &mut PostgresConnection,
        method: AuthMethod,
        credential_id: &str,
    ) -> Result<AuthContext, PlatformError> {
        let query = if method == AuthMethod::ApiKey {
            "SELECT a.id,a.kind,k.scope FROM api_keys k JOIN actors a ON a.id=k.actor_id WHERE k.id=$1 AND a.active AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at>clock_timestamp())"
        } else {
            "SELECT a.id,a.kind,NULL::jsonb FROM sessions s JOIN actors a ON a.id=s.actor_id JOIN users u ON u.actor_id=a.id WHERE s.access_stage='ready' AND s.id=$1 AND a.active AND a.kind='human' AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND s.absolute_expires_at>clock_timestamp()"
        };
        let row = client
            .client()
            .query_opt(query, &[&credential_id])
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Unauthorized)?;
        let actor = row.get::<_, String>(0);
        let kind: ActorKind = serde_json::from_value(Value::String(row.get(1)))
            .map_err(|_| PlatformError::Storage)?;
        let scope = row
            .get::<_, Option<Value>>(2)
            .map(serde_json::from_value::<KeyScope>)
            .transpose()
            .map_err(|_| PlatformError::Storage)?;
        if let Some(scope) = &scope {
            scope.validate()?;
        }

        let grants = client
            .client()
            .query(
                "SELECT capability,project_id FROM actor_grants WHERE actor_id=$1",
                &[&actor],
            )
            .map_err(|_| PlatformError::Storage)?
            .into_iter()
            .map(|row| {
                let capability = serde_json::from_value(Value::String(row.get(0)))
                    .map_err(|_| PlatformError::Storage)?;
                Ok(CapabilityGrant::new(capability, row.get(1)))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;

        Ok(AuthContext::new(
            actor,
            kind,
            method,
            credential_id.into(),
            grants,
            scope,
        ))
    }
}
