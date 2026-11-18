use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Deserialize, Serialize)]
pub(crate) struct GitlabTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub scope: String,
    pub expires_in: u64,
}
impl GitlabTokens {
    pub fn validate(&self) -> Result<(), nexora_platform::PlatformError> {
        let scopes: Vec<_> = self.scope.split_whitespace().collect();
        if self.access_token.is_empty()
            || self.access_token.len() > 65536
            || self.access_token.chars().any(char::is_control)
            || !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == 0
            || self.expires_in > 31_536_000
            || !(scopes.contains(&"api")
                || scopes.contains(&"read_api") && scopes.contains(&"read_repository"))
            || scopes
                .iter()
                .any(|scope| !["api", "read_api", "read_repository"].contains(scope))
        {
            return Err(nexora_platform::PlatformError::Forbidden);
        }
        Ok(())
    }
}
impl Drop for GitlabTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = self.refresh_token.as_mut() {
            token.zeroize();
        }
    }
}

#[cfg(test)]
#[path = "gitlab_token_permissions_tests.rs"]
mod tests;
