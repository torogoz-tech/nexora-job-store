use nexora_platform::PlatformError;
use sha2::{Digest, Sha256};
/// Generates real credentials from OS entropy; never prints or stores the secret.
pub struct SecretGenerator;
impl SecretGenerator {
    pub fn id(prefix: &str) -> Result<String, PlatformError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| PlatformError::Unavailable)?;
        Ok(format!("{prefix}_{}", Self::hex(&bytes)))
    }
    pub fn token(prefix: &str) -> Result<String, PlatformError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| PlatformError::Unavailable)?;
        Ok(format!("{prefix}_{}", Self::hex(&bytes)))
    }
    pub(crate) fn digest(token: &str) -> Vec<u8> {
        Sha256::digest(token.as_bytes()).to_vec()
    }
    pub(crate) fn checksum(bytes: &[u8]) -> String {
        Self::hex(&Sha256::digest(bytes))
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
