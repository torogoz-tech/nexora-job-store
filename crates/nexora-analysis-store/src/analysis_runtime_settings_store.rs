use crate::{
    AnalysisServiceRuntimeSettings, AnalysisStore, ProviderSecretContext,
    provider_secret_envelope::ProviderSecretEnvelope, secret_authority::SecretAuthority,
};
use nexora_platform::{OpenAiConfiguration, OpenAiEndpoint, PlatformError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl AnalysisStore {
    pub fn openai_runtime_settings(
        &mut self,
    ) -> Result<Option<AnalysisServiceRuntimeSettings>, PlatformError> {
        let row = self
            .client()
            .query_opt(
                "SELECT settings,secret,secret_revision,managed FROM installation_services WHERE kind='openai'",
                &[],
            )
            .map_err(Self::database_error)?
            .ok_or(PlatformError::NotFound)?;
        if !row.get::<_, bool>(3) {
            return Ok(None);
        }

        let value: Value = row.get(0);
        let settings: OpenAiConfiguration = Self::decode(value.clone())?;
        settings.validate()?;
        if !settings.enabled {
            return Err(PlatformError::Unavailable);
        }

        let secret = row
            .get::<_, Option<Value>>(1)
            .map(Self::decode::<ProviderSecretEnvelope>)
            .transpose()?
            .map(|envelope| {
                self.provider_secret_cipher()?
                    .open(&Self::secret_context(&value, row.get(2))?, &envelope)
            })
            .transpose()?;

        Ok(Some(AnalysisServiceRuntimeSettings { settings, secret }))
    }

    fn secret_context(
        settings: &Value,
        revision: i64,
    ) -> Result<ProviderSecretContext, PlatformError> {
        let endpoint = settings["endpoint"]
            .as_str()
            .ok_or(PlatformError::Storage)?;
        let endpoint = OpenAiEndpoint::normalize(endpoint)?;
        let binding = json!({"endpoint": endpoint});
        let host = format!("{:x}", Sha256::digest(binding.to_string().as_bytes()));

        Ok(ProviderSecretContext {
            binding_id: "openai-settings".into(),
            provider: SecretAuthority::OpenAi,
            host,
            purpose: "installation_service_secret".into(),
            revision,
        })
    }
}
