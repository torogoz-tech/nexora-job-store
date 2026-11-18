use nexora_platform::PlatformError;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Deserialize, Serialize)]
pub(crate) struct GithubTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub scope: String,
    pub expires_in: u64,
    pub refresh_token_expires_in: u64,
}
impl GithubTokens {
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.access_token.is_empty()
            || self.access_token.len() > 16384
            || self.access_token.chars().any(char::is_control)
            || self.refresh_token.is_empty()
            || self.refresh_token.len() > 16384
            || self.refresh_token.chars().any(char::is_control)
            || !self.token_type.eq_ignore_ascii_case("bearer")
            || !self.scope.is_empty()
            || self.expires_in == 0
            || self.expires_in > 86400
            || self.refresh_token_expires_in == 0
            || self.refresh_token_expires_in > 31_536_000
        {
            return Err(PlatformError::Unauthorized);
        }
        Ok(())
    }
}
impl Drop for GithubTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
