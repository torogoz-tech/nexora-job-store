use jsonwebtoken::{Algorithm, EncodingKey, Header};
use nexora_platform::PlatformError;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;
pub(crate) struct GithubJwt;
impl GithubJwt {
    pub fn sign(client_id: &str, pem: &str) -> Result<Zeroizing<String>, PlatformError> {
        let key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|_| PlatformError::Validation("invalid GitHub RSA private key"))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PlatformError::Unavailable)?
            .as_secs();
        jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &json!({"iss":client_id,"iat":now.saturating_sub(60),"exp":now+540}),
            &key,
        )
        .map(Zeroizing::new)
        .map_err(|_| PlatformError::Validation("GitHub JWT signing failed"))
    }
}
