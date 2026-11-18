use crate::provider_secret_envelope::ProviderSecretEnvelope;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use nexora_platform::{OpenAiConfiguration, OpenAiEndpoint, PlatformError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub(crate) fn open_openai_secret(
    master_key: &[u8; 32],
    namespace: &str,
    settings: &Value,
    secret_revision: i64,
    envelope: &ProviderSecretEnvelope,
) -> Result<Zeroizing<Vec<u8>>, PlatformError> {
    if envelope.key_version != 1
        || envelope.ciphertext.len() < 17
        || envelope.ciphertext.len() > 65_552
        || secret_revision < 1
    {
        return Err(PlatformError::Storage);
    }

    let configuration: OpenAiConfiguration = serde_json::from_value(settings.clone())
        .map_err(|_| PlatformError::Validation("invalid service configuration"))?;
    configuration.validate()?;
    let endpoint = OpenAiEndpoint::normalize(&configuration.endpoint)?;
    let binding = json!({"endpoint": endpoint});
    let binding = Sha256::digest(binding.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let associated_data = serde_json::to_vec(&(
        "nexora-provider-secret-v1",
        namespace,
        1_u32,
        "openai-settings",
        "openai",
        binding,
        "installation_service_secret",
        secret_revision,
    ))
    .map_err(|_| PlatformError::Storage)?;
    let hkdf = Hkdf::<Sha256>::new(Some(b"nexora-platform-hkdf-v1"), master_key);
    let mut key = Zeroizing::new([0_u8; 32]);
    let info = format!("nexora-provider-secrets/aes256gcm/v1:{namespace}");
    hkdf.expand(info.as_bytes(), key.as_mut())
        .map_err(|_| PlatformError::Unavailable)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| PlatformError::Unavailable)?;
    let plaintext = cipher
        .decrypt(
            &Nonce::from(envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &associated_data,
            },
        )
        .map_err(|_| PlatformError::Storage)?;
    Ok(Zeroizing::new(plaintext))
}
