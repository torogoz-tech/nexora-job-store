use crate::{
    GitNetworkPolicy, ProviderSecretContext, ProviderSecretEnvelope, TaskWorkflowStore,
    github_secrets::GithubSecrets,
};
use nexora_platform::{GitProvider, GithubConfiguration, PlatformError};
use serde_json::Value;

pub(crate) struct GithubSettingsStore<'a> {
    store: &'a mut TaskWorkflowStore,
}

impl<'a> GithubSettingsStore<'a> {
    pub(crate) fn new(store: &'a mut TaskWorkflowStore) -> Self {
        Self { store }
    }

    pub(crate) fn context(
        id: &str,
        host: &str,
        purpose: &str,
        revision: i64,
    ) -> ProviderSecretContext {
        ProviderSecretContext {
            binding_id: id.into(),
            provider: GitProvider::Github.into(),
            host: host.into(),
            purpose: purpose.into(),
            revision,
        }
    }

    pub(crate) fn load(
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
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::Unauthorized)?;
        let config: GithubConfiguration = TaskWorkflowStore::decode(row.get(0))?;
        if config.origin != policy.github_origin || config.api_origin != policy.github_api_origin {
            return Err(PlatformError::Conflict(
                "GitHub operator origin changed; reconfigure application",
            ));
        }
        let encrypted: ProviderSecretEnvelope = TaskWorkflowStore::decode(row.get(1))?;
        let plaintext = self.store.provider_secret_cipher()?.open(
            &Self::context(
                "github-settings",
                &config.host,
                "github_app_secrets",
                row.get(2),
            ),
            &encrypted,
        )?;
        let secrets: GithubSecrets = serde_json::from_slice::<Value>(&plaintext)
            .map_err(|_| PlatformError::Storage)
            .and_then(TaskWorkflowStore::decode)?;
        Ok((config, secrets))
    }
}
