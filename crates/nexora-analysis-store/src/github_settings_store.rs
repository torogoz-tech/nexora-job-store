use crate::{
    AnalysisStore, GitNetworkPolicy, ProviderSecretContext, ProviderSecretEnvelope,
    github_secrets::GithubSecrets,
};
use nexora_platform::{GitProvider, GithubConfiguration, PlatformError};

pub(crate) struct GithubSettingsStore<'a> {
    store: &'a mut AnalysisStore,
}

impl<'a> GithubSettingsStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }

    pub fn context(id: &str, host: &str, purpose: &str, revision: i64) -> ProviderSecretContext {
        ProviderSecretContext {
            binding_id: id.into(),
            provider: GitProvider::Github.into(),
            host: host.into(),
            purpose: purpose.into(),
            revision,
        }
    }

    pub fn load(
        &mut self,
        policy: &GitNetworkPolicy,
    ) -> Result<(GithubConfiguration, GithubSecrets), PlatformError> {
        let row = self
            .store
            .client()
            .query_opt(
                "SELECT to_jsonb(s)-'secrets'-'singleton',secrets,credential_revision FROM github_settings s WHERE singleton AND enabled",
                &[],
            )
            .map_err(AnalysisStore::database_error)?
            .ok_or(PlatformError::Unauthorized)?;
        let configuration: GithubConfiguration = AnalysisStore::decode(row.get(0))?;
        if configuration.origin != policy.github_origin
            || configuration.api_origin != policy.github_api_origin
        {
            return Err(PlatformError::Conflict(
                "GitHub operator origin changed; reconfigure application",
            ));
        }

        let encrypted: ProviderSecretEnvelope = AnalysisStore::decode(row.get(1))?;
        let plaintext = self.store.provider_secret_cipher()?.open(
            &Self::context(
                "github-settings",
                &configuration.host,
                "github_app_secrets",
                row.get(2),
            ),
            &encrypted,
        )?;
        let secrets = serde_json::from_slice(&plaintext).map_err(|_| PlatformError::Storage)?;
        Ok((configuration, secrets))
    }
}
