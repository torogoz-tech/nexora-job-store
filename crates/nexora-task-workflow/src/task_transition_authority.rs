use crate::TaskWorkflowStore;
use nexora_platform::{
    ActorKind, AuthContext, AuthMethod, CapabilityGrant, KeyScope, PlatformError,
};
use serde_json::Value;

/// Reloads the original credential without refreshing activity timestamps.
pub(crate) struct TaskTransitionAuthority;

impl TaskTransitionAuthority {
    pub(crate) fn reload_credential(
        store: &mut TaskWorkflowStore,
        method: AuthMethod,
        credential_id: &str,
    ) -> Result<AuthContext, PlatformError> {
        let query = match method {
            AuthMethod::ApiKey => {
                "SELECT a.id,a.kind,k.scope FROM api_keys k JOIN actors a ON a.id=k.actor_id WHERE k.id=$1 AND a.active AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at>now())"
            }
            AuthMethod::Session => {
                "SELECT a.id,a.kind,NULL::jsonb FROM sessions s JOIN actors a ON a.id=s.actor_id JOIN users u ON u.actor_id=a.id WHERE s.access_stage='ready' AND s.id=$1 AND a.active AND a.kind='human' AND s.revoked_at IS NULL AND s.expires_at>now() AND s.absolute_expires_at>now()"
            }
        };
        let row = store
            .client()
            .query_opt(query, &[&credential_id])
            .map_err(|_| PlatformError::Storage)?
            .ok_or(PlatformError::Unauthorized)?;
        let actor_id: String = row.get(0);
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
        let grants = store
            .client()
            .query(
                "SELECT capability,project_id FROM actor_grants WHERE actor_id=$1",
                &[&actor_id],
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
            actor_id,
            kind,
            method,
            credential_id.to_owned(),
            grants,
            scope,
        ))
    }

    pub(crate) fn matches_recorded_actor(context: &AuthContext, recorded_actor: &str) -> bool {
        context.actor_id() == recorded_actor
    }
}

#[cfg(test)]
mod tests {
    use super::TaskTransitionAuthority;
    use nexora_platform::{ActorKind, AuthContext, AuthMethod};

    #[test]
    fn rejects_a_reloaded_credential_for_a_different_actor() {
        let context = AuthContext::new(
            "act_current".into(),
            ActorKind::Human,
            AuthMethod::Session,
            "ses_1".into(),
            Vec::new(),
            None,
        );

        assert!(!TaskTransitionAuthority::matches_recorded_actor(
            &context,
            "act_recorded"
        ));
    }

    #[test]
    fn accepts_only_the_recorded_actor() {
        let context = AuthContext::new(
            "act_1".into(),
            ActorKind::Human,
            AuthMethod::ApiKey,
            "key_1".into(),
            Vec::new(),
            None,
        );

        assert!(TaskTransitionAuthority::matches_recorded_actor(
            &context, "act_1"
        ));
    }
}
