use crate::{GitNetworkPolicy, gitlab_tokens::GitlabTokens};
use nexora_platform::{GitProvider, GitlabConfiguration, PlatformError, RepositoryRegistration};
use serde_json::Value;
pub(crate) struct GitlabClient {
    agent: ureq::Agent,
    policy: GitNetworkPolicy,
}
impl GitlabClient {
    pub fn commit(
        &self,
        token: &str,
        id: &str,
        pinned: Option<&str>,
    ) -> Result<String, PlatformError> {
        if pinned.is_some_and(|oid| !crate::git_commit_identity::GitCommitIdentity::valid(oid)) {
            return Err(PlatformError::Conflict("invalid pinned commit"));
        }
        let path = format!("/api/v4/projects/{id}/repository/commits");
        let mut uri = self.policy.url(&path)?;
        if let Some(oid) = pinned {
            uri.path_segments_mut()
                .map_err(|_| PlatformError::Validation("invalid Git endpoint"))?
                .push(oid);
        } else {
            uri.query_pairs_mut().append_pair("per_page", "1");
        }
        let value = self.get(uri, token)?;
        crate::git_commit_identity::GitCommitIdentity::response(&value, "id", pinned)
    }
    pub fn new(policy: &GitNetworkPolicy) -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .proxy(None)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(10)))
                .build()
                .into(),
            policy: policy.clone(),
        }
    }
    pub fn refresh(
        &self,
        config: &GitlabConfiguration,
        secret: &str,
        refresh: &str,
    ) -> Result<GitlabTokens, PlatformError> {
        self.token_request(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ])
    }
    fn token_request(&self, form: &[(&str, &str)]) -> Result<GitlabTokens, PlatformError> {
        let mut response = self
            .agent
            .post(self.policy.url("/oauth/token")?.as_str())
            .header("Accept", "application/json")
            .send_form(form.iter().copied())
            .map_err(|_| PlatformError::Unavailable)?;
        if !response.status().is_success() {
            return Err(if response.status().as_u16() >= 500 {
                PlatformError::Unavailable
            } else {
                PlatformError::Unauthorized
            });
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1_048_576)
            .read_to_vec()
            .map_err(|_| PlatformError::Unavailable)?;
        let tokens: GitlabTokens =
            serde_json::from_slice(&bytes).map_err(|_| PlatformError::Unauthorized)?;
        tokens.validate()?;
        Ok(tokens)
    }
    pub(crate) fn get(&self, uri: url::Url, token: &str) -> Result<Value, PlatformError> {
        let mut response = self
            .agent
            .get(uri.as_str())
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .map_err(|_| PlatformError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 => return Err(PlatformError::Unauthorized),
            403 => return Err(PlatformError::Forbidden),
            404 => return Err(PlatformError::NotFound),
            _ => return Err(PlatformError::Unavailable),
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1_048_576)
            .read_to_vec()
            .map_err(|_| PlatformError::Unavailable)?;
        serde_json::from_slice(&bytes).map_err(|_| PlatformError::Unavailable)
    }
    pub fn user(&self, token: &str) -> Result<(String, String), PlatformError> {
        let value = self.get(self.policy.url("/api/v4/user")?, token)?;
        let id = value["id"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or(PlatformError::Unauthorized)?;
        let username = value["username"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 160)
            .ok_or(PlatformError::Unauthorized)?;
        if value["state"].as_str().is_some_and(|s| s != "active") {
            return Err(PlatformError::Unauthorized);
        }
        Ok((id.to_string(), username.into()))
    }
    pub fn repository(
        &self,
        token: &str,
        locator: &str,
    ) -> Result<(String, String), PlatformError> {
        let mut uri = self.policy.url("/api/v4/projects/")?;
        uri.path_segments_mut()
            .map_err(|_| PlatformError::Validation("invalid Git endpoint"))?
            .pop_if_empty()
            .push(locator);
        let value = self.get(uri, token)?;
        let id = value["id"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or(PlatformError::Unavailable)?
            .to_string();
        let name = value["path_with_namespace"]
            .as_str()
            .ok_or(PlatformError::Unavailable)?;
        RepositoryRegistration {
            provider: GitProvider::Gitlab,
            full_name: name.into(),
        }
        .locator()?;
        if value["repository_access_level"] == "disabled" {
            return Err(PlatformError::Forbidden);
        }
        let mut branches = self
            .policy
            .url(&format!("/api/v4/projects/{id}/repository/branches"))?;
        branches.query_pairs_mut().append_pair("per_page", "1");
        if !self.get(branches, token)?.is_array() {
            return Err(PlatformError::Unavailable);
        }
        Ok((id, name.into()))
    }
}
