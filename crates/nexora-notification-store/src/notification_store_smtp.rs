use crate::{
    NotificationDeliveryRuntime, NotificationStore, provider_secret_cipher::ProviderSecretCipher,
    provider_secret_context::ProviderSecretContext,
    provider_secret_envelope::ProviderSecretEnvelope,
};
use nexora_platform::{InstallationService, PlatformError, SmtpConfiguration};
use serde_json::Value;

pub(crate) struct NotificationStoreSmtp;

impl NotificationStoreSmtp {
    pub(crate) fn runtime_settings(
        store: &mut NotificationStore,
    ) -> Result<Option<NotificationDeliveryRuntime>, PlatformError> {
        let row = store
            .client()
            .query_one(
                "SELECT settings,secret,secret_revision,managed FROM installation_services WHERE kind=$1",
                &[&InstallationService::Smtp.as_str()],
            )
            .map_err(NotificationStore::database_error)?;
        if !row.get::<_, bool>(3) {
            return Ok(None);
        }

        let settings: Value = row.get(0);
        let configuration: SmtpConfiguration =
            serde_json::from_value(settings.clone()).map_err(|_| PlatformError::Storage)?;
        configuration.validate()?;

        let encrypted: Option<Value> = row.get(1);
        let secret = encrypted
            .map(|value| Self::decrypt_secret(store, settings.clone(), row.get(2), value))
            .transpose()?;

        Ok(Some(NotificationDeliveryRuntime { settings, secret }))
    }

    fn decrypt_secret(
        store: &mut NotificationStore,
        settings: Value,
        revision: i64,
        value: Value,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, PlatformError> {
        let cipher = ProviderSecretCipher::load(&store.secrets_directory, &store.namespace)?;
        let crypto_state = store
            .client()
            .query_one(
                "SELECT key_version,key_id FROM platform_crypto_state WHERE singleton",
                &[],
            )
            .map_err(NotificationStore::database_error)?;
        if crypto_state.get::<_, i32>(0) != 1 || crypto_state.get::<_, String>(1) != cipher.key_id()
        {
            return Err(PlatformError::Conflict("installation master key mismatch"));
        }

        let context = ProviderSecretContext::smtp(&settings, revision);
        let envelope: ProviderSecretEnvelope =
            serde_json::from_value(value).map_err(|_| PlatformError::Storage)?;
        cipher.open(&context, envelope)
    }
}
