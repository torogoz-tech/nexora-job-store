use crate::{
    KnowledgeServiceRuntimeSettings, KnowledgeStore,
    knowledge_store_master_key::KnowledgeStoreMasterKey,
    knowledge_store_secret_cipher::open_openai_secret,
    provider_secret_envelope::ProviderSecretEnvelope,
};
use nexora_platform::PlatformError;
use serde_json::Value;

impl KnowledgeStore {
    pub fn openai_service_runtime_settings(
        &mut self,
    ) -> Result<Option<KnowledgeServiceRuntimeSettings>, PlatformError> {
        let row = self
            .client()
            .query_one(
                "SELECT settings,secret,secret_revision,managed FROM installation_services WHERE kind='openai'",
                &[],
            )
            .map_err(Self::database_error)?;
        if !row.get::<_, bool>(3) {
            return Ok(None);
        }

        let settings: Value = row.get(0);
        let configuration =
            serde_json::from_value::<nexora_platform::OpenAiConfiguration>(settings.clone())
                .map_err(|_| PlatformError::Validation("invalid service configuration"))?;
        configuration.validate()?;
        if !configuration.enabled {
            return Err(PlatformError::Unavailable);
        }

        let encrypted: Option<Value> = row.get(1);
        let secret_revision: i64 = row.get(2);
        let secret = encrypted
            .map(|value| {
                let envelope: ProviderSecretEnvelope =
                    serde_json::from_value(value).map_err(|_| PlatformError::Storage)?;
                let master_key =
                    KnowledgeStoreMasterKey::load(&self.secrets_directory, &self.namespace)?;
                let key_row = self
                    .client()
                    .query_opt(
                        "SELECT key_version,key_id FROM platform_crypto_state WHERE singleton",
                        &[],
                    )
                    .map_err(Self::database_error)?
                    .ok_or(PlatformError::Storage)?;
                if key_row.get::<_, i32>(0) != 1 || key_row.get::<_, String>(1) != master_key.id() {
                    return Err(PlatformError::Conflict("installation master key mismatch"));
                }
                open_openai_secret(
                    master_key.bytes(),
                    &self.namespace,
                    &settings,
                    secret_revision,
                    &envelope,
                )
            })
            .transpose()?;

        Ok(Some(KnowledgeServiceRuntimeSettings { settings, secret }))
    }
}
