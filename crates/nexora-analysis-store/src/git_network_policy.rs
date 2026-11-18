use nexora_platform::{GitlabClientRegistration, PlatformError};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitNetworkPolicy {
    pub gitlab_origin: String,
    pub callback_origin: String,
    #[serde(default = "GitNetworkPolicy::default_github_origin")]
    pub github_origin: String,
    #[serde(default = "GitNetworkPolicy::default_github_api_origin")]
    pub github_api_origin: String,
}
impl Default for GitNetworkPolicy {
    fn default() -> Self {
        Self {
            gitlab_origin: "https://gitlab.com".into(),
            callback_origin: "http://127.0.0.1:18080".into(),
            github_origin: Self::default_github_origin(),
            github_api_origin: Self::default_github_api_origin(),
        }
    }
}
impl GitNetworkPolicy {
    fn default_github_origin() -> String {
        "https://github.com".into()
    }
    fn default_github_api_origin() -> String {
        "https://api.github.com".into()
    }
    pub fn load(path: &Path) -> Result<Self, PlatformError> {
        let bytes = std::fs::read(path).map_err(|_| PlatformError::Storage)?;
        if bytes.len() > 65536 {
            return Err(PlatformError::Validation("Git policy exceeds limit"));
        }
        let policy: Self = serde_json::from_slice(&bytes)
            .map_err(|_| PlatformError::Validation("invalid Git policy"))?;
        policy.validate()?;
        Ok(policy)
    }
    pub fn validate(&self) -> Result<(), PlatformError> {
        Self::origin(&self.gitlab_origin)?;
        Self::origin(&self.github_origin)?;
        Self::origin(&self.github_api_origin)?;
        Self::origin(&self.callback_origin)?;
        Ok(())
    }
    pub(crate) fn origin(value: &str) -> Result<url::Url, PlatformError> {
        let uri =
            url::Url::parse(value).map_err(|_| PlatformError::Validation("invalid Git origin"))?;
        let local = uri
            .host_str()
            .and_then(|h| h.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if value.len() > 2048
            || uri.origin().ascii_serialization() != value
            || uri.path() != "/"
            || uri.query().is_some()
            || uri.fragment().is_some()
            || !uri.username().is_empty()
            || uri.password().is_some()
            || !(uri.scheme() == "https" || uri.scheme() == "http" && local)
        {
            return Err(PlatformError::Validation(
                "Git policy requires HTTPS origins or explicit IP loopback HTTP",
            ));
        }
        Ok(uri)
    }
    pub fn github_url(&self, path: &str, api: bool) -> Result<url::Url, PlatformError> {
        let base = Self::origin(if api {
            &self.github_api_origin
        } else {
            &self.github_origin
        })?;
        let url = base
            .join(path)
            .map_err(|_| PlatformError::Validation("invalid GitHub endpoint"))?;
        if url.origin() != base.origin() {
            return Err(PlatformError::Validation(
                "GitHub endpoint outside operator policy",
            ));
        }
        Ok(url)
    }
    pub fn github_host(&self) -> Result<String, PlatformError> {
        let uri = Self::origin(&self.github_origin)?;
        Ok(match uri.port() {
            Some(port) => format!("{}:{port}", uri.host_str().unwrap_or("")),
            None => uri.host_str().unwrap_or("").into(),
        })
    }
    pub fn host(&self) -> Result<String, PlatformError> {
        let uri = Self::origin(&self.gitlab_origin)?;
        Ok(match uri.port() {
            Some(port) => format!("{}:{port}", uri.host_str().unwrap_or("")),
            None => uri.host_str().unwrap_or("").into(),
        })
    }
    pub fn validate_registration(
        &self,
        input: &GitlabClientRegistration,
    ) -> Result<(), PlatformError> {
        self.validate()?;
        if input.client_id.is_empty()
            || input.client_id.len() > 512
            || input.client_secret.is_empty()
            || input.client_secret.len() > 8192
            || input.expected_revision < 0
            || input.redirect_uri != format!("{}/api/v1/git/gitlab/callback", self.callback_origin)
        {
            return Err(PlatformError::Validation(
                "invalid GitLab client registration or callback",
            ));
        }
        Ok(())
    }
    pub fn url(&self, path: &str) -> Result<url::Url, PlatformError> {
        let base = Self::origin(&self.gitlab_origin)?;
        let result = base
            .join(path)
            .map_err(|_| PlatformError::Validation("invalid Git endpoint"))?;
        if result.origin() != base.origin() {
            return Err(PlatformError::Validation(
                "Git endpoint outside operator policy",
            ));
        }
        Ok(result)
    }
}
