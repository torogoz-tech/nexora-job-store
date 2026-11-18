use crate::{AnalysisStore, GitNetworkPolicy, ProviderSecretContext, ProviderSecretEnvelope};
use nexora_platform::{GitProvider, GitlabConfiguration, PlatformError};
use zeroize::Zeroizing;

pub(crate) struct GitlabSettingsStore<'a> {
    store: &'a mut AnalysisStore,
}

impl<'a> GitlabSettingsStore<'a> {
    pub fn new(store: &'a mut AnalysisStore) -> Self {
        Self { store }
    }

    pub fn context(id: &str, host: &str, purpose: &str, revision: i64) -> ProviderSecretContext {
        ProviderSecretContext {
            binding_id: id.into(),
            host: host.into(),
            provider: GitProvider::Gitlab.into(),
            purpose: purpose.into(),
            revision,
        }
    }

    pub fn load(
        &mut self,
        policy: &GitNetworkPolicy,
    ) -> Result<(GitlabConfiguration, Zeroizing<Vec<u8>>), PlatformError> {
        let row = self
            .store
            .client()
            .query_opt(
                "SELECT to_jsonb(s)-'client_secret'-'singleton',client_secret,credential_revision FROM gitlab_settings s WHERE singleton AND enabled",
                &[],
            )
            .map_err(AnalysisStore::database_error)?
            .ok_or(PlatformError::Unauthorized)?;
        let configuration: GitlabConfiguration = AnalysisStore::decode(row.get(0))?;
        if configuration.origin != policy.gitlab_origin {
            return Err(PlatformError::Conflict(
                "GitLab operator origin changed; reconfigure application",
            ));
        }

        let encrypted: ProviderSecretEnvelope = AnalysisStore::decode(row.get(1))?;
        let secret = self.store.provider_secret_cipher()?.open(
            &Self::context(
                "gitlab-settings",
                &configuration.host,
                "gitlab_client_secret",
                row.get(2),
            ),
            &encrypted,
        )?;
        Ok((configuration, secret))
    }
}
