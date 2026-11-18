use crate::{
    AnalysisStore, GitNetworkPolicy, ProviderSecretEnvelope, audit_writer::AuditWriter,
    github_client::GithubClient, github_settings_store::GithubSettingsStore,
    github_tokens::GithubTokens,
};
use nexora_platform::*;
use serde_json::json;
use zeroize::Zeroizing;
pub(crate) struct GithubTokenStore<'a> {
    store: &'a mut AnalysisStore,
}
impl<'a> GithubTokenStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }
    pub fn tokens(
        &mut self,
        connection: &GitConnection,
        policy: &GitNetworkPolicy,
    ) -> Result<GithubTokens, PlatformError> {
        let (config, secret) = GithubSettingsStore::new(self.store).load(policy)?;
        if config.host != connection.host {
            return Err(PlatformError::Unauthorized);
        }
        let cipher = self.store.provider_secret_cipher()?;
        let mut tx = self
            .store
            .client()
            .transaction()
            .map_err(AnalysisStore::database_error)?;
        let row=tx.query_opt("SELECT c.tokens,c.credential_revision,c.expires_at>now()+interval '30 seconds' FROM git_connections c JOIN actors a ON a.id=c.owner_actor_id WHERE c.id=$1 AND c.status='active' AND c.provider='github' AND a.active FOR UPDATE OF c",&[&connection.id]).map_err(AnalysisStore::database_error)?.ok_or(PlatformError::Unauthorized)?;
        let encrypted: ProviderSecretEnvelope = AnalysisStore::decode(row.get(0))?;
        let revision: i64 = row.get(1);
        let active: bool = row.get(2);
        let plain = cipher.open(
            &GithubSettingsStore::context(
                &connection.id,
                &connection.host,
                "github_user_tokens",
                revision,
            ),
            &encrypted,
        )?;
        let tokens: GithubTokens =
            serde_json::from_slice(&plain).map_err(|_| PlatformError::Storage)?;
        tokens.validate()?;
        let client = GithubClient::new(policy);
        if active {
            // Rotated credentials can be durable before the provider's identity
            // endpoint recovers. Every use must still prove the bound account.
            if client.user(&tokens.access_token)?.0 != connection.remote_account_id {
                return Err(PlatformError::Unauthorized);
            }
            tx.commit().map_err(AnalysisStore::database_error)?;
            return Ok(tokens);
        }
        let next = client.refresh(&config, &secret, &tokens.refresh_token)?;
        let plain = Zeroizing::new(serde_json::to_vec(&next).map_err(|_| PlatformError::Storage)?);
        let encrypted = serde_json::to_value(cipher.seal(
            &GithubSettingsStore::context(
                &connection.id,
                &connection.host,
                "github_user_tokens",
                revision + 1,
            ),
            &plain,
        )?)
        .map_err(|_| PlatformError::Storage)?;
        tx.execute("UPDATE git_connections SET tokens=$2,credential_revision=credential_revision+1,config_revision=$3,expires_at=now()+$4::bigint*interval '1 second',updated_at=clock_timestamp() WHERE id=$1",&[&connection.id,&encrypted,&config.revision,&(next.expires_in as i64)]).map_err(AnalysisStore::database_error)?;
        AuditWriter::append(
            &mut tx,
            None,
            None,
            "git.credential_refreshed",
            Some(&connection.id),
            json!({"credential_revision":revision+1}),
        )?;
        tx.commit().map_err(AnalysisStore::database_error)?;
        // A successful refresh consumes the old refresh token at the provider.
        // Preserve the replacement even if identity verification is unavailable,
        // but never return it without checking the original account binding.
        if client.user(&next.access_token)?.0 != connection.remote_account_id {
            return Err(PlatformError::Unauthorized);
        }
        Ok(next)
    }
}
