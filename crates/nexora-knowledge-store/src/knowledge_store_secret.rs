use nexora_platform::PlatformError;
use sha2::{Digest, Sha256};

pub(crate) struct KnowledgeStoreSecret;

impl KnowledgeStoreSecret {
    pub(crate) fn token(prefix: &str) -> Result<String, PlatformError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| PlatformError::Unavailable)?;
        Ok(format!("{prefix}_{}", Self::hex(&bytes)))
    }

    pub(crate) fn digest(value: &str) -> Vec<u8> {
        Sha256::digest(value.as_bytes()).to_vec()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
