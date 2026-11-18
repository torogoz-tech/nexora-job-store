use crate::GitNetworkPolicy;
use nexora_platform::{GitProvider, PlatformError};
use serde_json::Value;
pub(crate) struct PullRequestClient {
    agent: ureq::Agent,
    policy: GitNetworkPolicy,
    provider: GitProvider,
}
impl PullRequestClient {
    pub fn new(policy: &GitNetworkPolicy, provider: GitProvider) -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .proxy(None)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(10)))
                .build()
                .into(),
            policy: policy.clone(),
            provider,
        }
    }
    pub fn post(&self, path: &str, token: &str, body: &Value) -> Result<Value, PlatformError> {
        let uri = match self.provider {
            GitProvider::Github => self.policy.github_url(path, true)?,
            GitProvider::Gitlab => self.policy.url(path)?,
        };
        let mut response = self
            .agent
            .post(uri.as_str())
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "Nexora/0.1")
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|_| PlatformError::Unavailable)?;
        match response.status().as_u16() {
            201 => {}
            401 => {
                return Err(PlatformError::Conflict(
                    "Git provider authorization requires reconnection",
                ));
            }
            403 => return Err(PlatformError::Forbidden),
            404 => return Err(PlatformError::NotFound),
            400 | 409 | 422 => {
                return Err(PlatformError::Validation(
                    "Git provider rejected creation; review branch rules and permissions",
                ));
            }
            429 => {
                return Err(PlatformError::Conflict(
                    "Git provider rate limit reached; retry after the provider allows requests",
                ));
            }
            _ => return Err(PlatformError::Unavailable),
        }
        let data = response
            .body_mut()
            .with_config()
            .limit(1_048_576)
            .read_to_vec()
            .map_err(|_| PlatformError::Unavailable)?;
        serde_json::from_slice(&data).map_err(|_| PlatformError::Unavailable)
    }
    pub fn put(&self, path: &str, token: &str, body: &Value) -> Result<Value, PlatformError> {
        let uri = match self.provider {
            GitProvider::Github => self.policy.github_url(path, true)?,
            GitProvider::Gitlab => self.policy.url(path)?,
        };
        let mut response = self
            .agent
            .put(uri.as_str())
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "Nexora/0.1")
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|_| PlatformError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 => {
                return Err(PlatformError::Conflict(
                    "Git provider authorization requires reconnection",
                ));
            }
            403 => return Err(PlatformError::Forbidden),
            404 => return Err(PlatformError::NotFound),
            400 | 405 | 409 | 422 => {
                return Err(PlatformError::Conflict(
                    "Git provider rejected the pinned merge; inspect approvals, checks and current source revision",
                ));
            }
            _ => return Err(PlatformError::Unavailable),
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1048576)
            .read_to_vec()
            .map_err(|_| PlatformError::Unavailable)?;
        serde_json::from_slice(&bytes).map_err(|_| PlatformError::Unavailable)
    }
    /// Protection writes use the provider's documented 200/201 responses.
    /// Callers must reconcile with GET after ambiguous transport outcomes.
    pub fn get(&self, path: &str, token: &str) -> Result<Value, PlatformError> {
        let uri = match self.provider {
            GitProvider::Github => self.policy.github_url(path, true)?,
            GitProvider::Gitlab => self.policy.url(path)?,
        };
        let mut response = self
            .agent
            .get(uri.as_str())
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "Nexora/0.1")
            .header("Accept", "application/json")
            .call()
            .map_err(|_| PlatformError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 => {
                return Err(PlatformError::Conflict(
                    "Git provider authorization requires reconnection",
                ));
            }
            403 => return Err(PlatformError::Forbidden),
            404 => return Err(PlatformError::NotFound),
            _ => return Err(PlatformError::Unavailable),
        }
        let data = response
            .body_mut()
            .with_config()
            .limit(1_048_576)
            .read_to_vec()
            .map_err(|_| PlatformError::Unavailable)?;
        serde_json::from_slice(&data).map_err(|_| PlatformError::Unavailable)
    }
}

#[cfg(test)]
#[path = "pull_request_client_tests.rs"]
mod tests;
