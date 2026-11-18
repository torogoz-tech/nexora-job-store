use crate::{
    TaskWorkflowStore, master_key::MasterKey, provider_secret_cipher::ProviderSecretCipher,
};
use nexora_platform::PlatformError;

impl TaskWorkflowStore {
    pub(crate) fn provider_secret_cipher(&mut self) -> Result<ProviderSecretCipher, PlatformError> {
        let key: MasterKey = self.installation_master_key()?;
        Ok(ProviderSecretCipher::new(key, &self.namespace))
    }
}
