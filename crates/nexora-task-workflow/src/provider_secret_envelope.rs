use serde::{Deserialize, Serialize};
/// Encrypted bytes only. Plaintext is deliberately absent from this representation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSecretEnvelope {
    pub key_version: u32,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}
