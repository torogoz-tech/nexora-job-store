use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderSecretEnvelope {
    pub key_version: u32,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}
