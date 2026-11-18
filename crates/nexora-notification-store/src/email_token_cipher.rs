use crate::{email_token_envelope::EmailTokenEnvelope, master_key::MasterKey};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use nexora_platform::PlatformError;
use serde_json::Value;
use zeroize::Zeroizing;

pub(crate) struct EmailTokenCipher {
    master: MasterKey,
    namespace: String,
}

impl EmailTokenCipher {
    pub(crate) fn new(master: MasterKey, namespace: &str) -> Result<Self, PlatformError> {
        Ok(Self {
            master,
            namespace: namespace.to_owned(),
        })
    }

    pub(crate) fn open(
        &self,
        id: &str,
        revision: i64,
        value: Value,
    ) -> Result<Zeroizing<String>, PlatformError> {
        if id.is_empty() || revision < 1 {
            return Err(PlatformError::Storage);
        }
        let envelope: EmailTokenEnvelope =
            serde_json::from_value(value).map_err(|_| PlatformError::Storage)?;
        let key = self.master.email_action_key(&self.namespace)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| PlatformError::Storage)?;
        let aad = format!("nexora-email-token-v1:{}:{id}:{revision}", self.namespace);
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    &Nonce::from(envelope.nonce),
                    Payload {
                        msg: &envelope.ciphertext,
                        aad: aad.as_bytes(),
                    },
                )
                .map_err(|_| PlatformError::Storage)?,
        );
        String::from_utf8(plaintext.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| PlatformError::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Payload;
    use serde_json::json;

    #[test]
    fn opens_api_compatible_email_action_envelope_and_rejects_changed_context() {
        let directory = std::env::temp_dir().join(format!(
            "nexora-email-token-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let namespace = "nexora_platform_test";
        crate::test_support::write_master_key(&directory, namespace).expect("master key");
        let master = MasterKey::load(&MasterKey::path(&directory, namespace)).expect("key");
        let key = master.email_action_key(namespace).expect("derived key");
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("AES key");
        let nonce = [9u8; 12];
        let id = "email-change-1";
        let revision = 1;
        let aad = format!("nexora-email-token-v1:{namespace}:{id}:{revision}");
        let ciphertext = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: b"123456",
                    aad: aad.as_bytes(),
                },
            )
            .expect("encrypted token");
        let envelope = json!({"nonce": nonce, "ciphertext": ciphertext});
        let decryptor = EmailTokenCipher::new(master, namespace).expect("decryptor");
        let token = decryptor
            .open(id, revision, envelope.clone())
            .expect("decrypt");
        assert_eq!(token.as_str(), "123456");
        assert!(decryptor.open(id, revision + 1, envelope).is_err());
        std::fs::remove_dir_all(directory).expect("temporary key cleanup");
    }
}
