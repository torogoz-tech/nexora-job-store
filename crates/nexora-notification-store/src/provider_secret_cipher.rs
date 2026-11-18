use crate::{
    master_key::MasterKey, provider_secret_context::ProviderSecretContext,
    provider_secret_envelope::ProviderSecretEnvelope,
};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use nexora_platform::PlatformError;
use std::path::Path;
use zeroize::Zeroizing;

pub(crate) struct ProviderSecretCipher {
    master: MasterKey,
    namespace: String,
}

impl ProviderSecretCipher {
    pub(crate) fn load(directory: &Path, namespace: &str) -> Result<Self, PlatformError> {
        if namespace.len() > 63
            || !namespace.starts_with("nexora_platform")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PlatformError::Validation("invalid platform namespace"));
        }
        let master = MasterKey::load(&MasterKey::path(directory, namespace))?;
        Ok(Self {
            master,
            namespace: namespace.into(),
        })
    }

    pub(crate) fn key_id(&self) -> String {
        self.master.id()
    }

    pub(crate) fn open(
        &self,
        context: &ProviderSecretContext,
        envelope: ProviderSecretEnvelope,
    ) -> Result<Zeroizing<Vec<u8>>, PlatformError> {
        if envelope.key_version != 1
            || envelope.ciphertext.len() < 17
            || envelope.ciphertext.len() > 65_552
            || context.revision < 1
        {
            return Err(PlatformError::Storage);
        }

        let aad = serde_json::to_vec(&(
            "nexora-provider-secret-v1",
            &self.namespace,
            1u32,
            &context.binding_id,
            "smtp",
            &context.host,
            "installation_service_secret",
            context.revision,
        ))
        .map_err(|_| PlatformError::Storage)?;
        let key = self.master.provider_key(&self.namespace)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| PlatformError::Storage)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_secret_context::ProviderSecretContext;
    use aes_gcm::aead::Payload;
    use serde_json::json;

    #[test]
    fn opens_api_compatible_smtp_secret_and_binds_it_to_settings() {
        let directory = std::env::temp_dir().join(format!(
            "nexora-provider-secret-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let namespace = "nexora_platform_test";
        crate::test_support::write_master_key(&directory, namespace).expect("master key");
        let master = MasterKey::load(&MasterKey::path(&directory, namespace)).expect("key");
        let key = master.provider_key(namespace).expect("derived key");
        let settings = json!({"host":"smtp.example.test","port":587,"username":"nexora","tls_mode":"starttls"});
        let context = ProviderSecretContext::smtp(&settings, 3);
        let aad = serde_json::to_vec(&(
            "nexora-provider-secret-v1",
            namespace,
            1u32,
            &context.binding_id,
            "smtp",
            &context.host,
            "installation_service_secret",
            context.revision,
        ))
        .expect("associated data");
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("AES key");
        let nonce = [5u8; 12];
        let ciphertext = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: b"secret-value",
                    aad: &aad,
                },
            )
            .expect("encrypted secret");
        let envelope = ProviderSecretEnvelope {
            key_version: 1,
            nonce,
            ciphertext,
        };
        let decryptor = ProviderSecretCipher::load(&directory, namespace).expect("decryptor");
        assert_eq!(
            decryptor
                .open(&context, envelope)
                .expect("decrypt")
                .as_slice(),
            b"secret-value"
        );

        let mut changed_settings = settings;
        changed_settings["port"] = json!(465);
        let changed_context = ProviderSecretContext::smtp(&changed_settings, 3);
        let encrypted = serde_json::to_value(ProviderSecretEnvelope {
            key_version: 1,
            nonce,
            ciphertext: cipher
                .encrypt(
                    &Nonce::from(nonce),
                    Payload {
                        msg: b"secret-value",
                        aad: &aad,
                    },
                )
                .expect("encrypted fixture"),
        })
        .expect("envelope");
        let encrypted: ProviderSecretEnvelope =
            serde_json::from_value(encrypted).expect("decode envelope");
        assert!(decryptor.open(&changed_context, encrypted).is_err());
        std::fs::remove_dir_all(directory).expect("temporary key cleanup");
    }
}
