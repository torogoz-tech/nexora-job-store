use crate::{ProviderSecretContext, ProviderSecretEnvelope, master_key::MasterKey};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use nexora_platform::PlatformError;
use zeroize::Zeroizing;
/// Trusted-host credential encryption. This is not an authorization or provider connector.
/// The caller must resolve its protected directory and validate the installation key binding.
pub struct ProviderSecretCipher {
    master: MasterKey,
    namespace: String,
}
impl ProviderSecretCipher {
    pub(crate) fn new(master: MasterKey, namespace: &str) -> Self {
        Self {
            master,
            namespace: namespace.into(),
        }
    }
    pub fn seal(
        &self,
        context: &ProviderSecretContext,
        plaintext: &[u8],
    ) -> Result<ProviderSecretEnvelope, PlatformError> {
        if plaintext.is_empty() || plaintext.len() > 65_536 {
            return Err(PlatformError::Validation(
                "provider secret must contain 1..65536 bytes",
            ));
        }
        let aad = self.associated_data(context)?;
        let key = self.master.provider_key(&self.namespace)?;
        let cipher =
            Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| PlatformError::Unavailable)?;
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| PlatformError::Unavailable)?;
        let ciphertext = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| PlatformError::Unavailable)?;
        Ok(ProviderSecretEnvelope {
            key_version: 1,
            nonce,
            ciphertext,
        })
    }
    pub fn open(
        &self,
        context: &ProviderSecretContext,
        envelope: &ProviderSecretEnvelope,
    ) -> Result<Zeroizing<Vec<u8>>, PlatformError> {
        if envelope.key_version != 1
            || envelope.ciphertext.len() < 17
            || envelope.ciphertext.len() > 65_552
        {
            return Err(PlatformError::Storage);
        }
        let aad = self.associated_data(context)?;
        let key = self.master.provider_key(&self.namespace)?;
        let cipher =
            Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| PlatformError::Unavailable)?;
        let plaintext = cipher
            .decrypt(
                &Nonce::from(envelope.nonce),
                Payload {
                    msg: &envelope.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| PlatformError::Storage)?;
        Ok(Zeroizing::new(plaintext))
    }
    fn associated_data(&self, context: &ProviderSecretContext) -> Result<Vec<u8>, PlatformError> {
        if context.revision < 1
            || context.binding_id.is_empty()
            || context.binding_id.len() > 128
            || context.host.is_empty()
            || context.host.len() > 253
            || context.purpose.is_empty()
            || context.purpose.len() > 64
            || [&context.binding_id, &context.host, &context.purpose]
                .iter()
                .any(|s| s.chars().any(char::is_control))
        {
            return Err(PlatformError::Validation("invalid provider secret context"));
        }
        // Frozen tuple order is part of envelope v1. Add fields only through a new format version.
        serde_json::to_vec(&(
            "nexora-provider-secret-v1",
            &self.namespace,
            1u32,
            &context.binding_id,
            context.provider.as_str(),
            &context.host,
            &context.purpose,
            context.revision,
        ))
        .map_err(|_| PlatformError::Storage)
    }
}
