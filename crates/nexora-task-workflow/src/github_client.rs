use crate::{
    GitNetworkPolicy, github_jwt::GithubJwt, github_repository_token::GithubRepositoryToken,
    github_secrets::GithubSecrets, github_tokens::GithubTokens,
};
use nexora_platform::*;
use serde_json::{Value, json};
use zeroize::Zeroizing;
pub(crate) struct GithubClient {
    agent: ureq::Agent,
    policy: GitNetworkPolicy,
}
impl GithubClient {
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
    fn response(mut response: ureq::http::Response<ureq::Body>) -> Result<Value, PlatformError> {
        if response.status().as_u16() == 403 {
            let limited = response.headers().contains_key("retry-after")
                || response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|value| value.to_str().ok())
                    == Some("0");
            if limited {
                return Err(PlatformError::Unavailable);
            }
            let bytes = response
                .body_mut()
                .with_config()
                .limit(1_048_576)
                .read_to_vec()
                .map_err(|_| PlatformError::Unavailable)?;
            let limited = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value["message"].as_str().map(str::to_ascii_lowercase))
                .is_some_and(|message| message.contains("rate limit"));
            return Err(if limited {
                PlatformError::Unavailable
            } else {
                PlatformError::Forbidden
            });
        }
        match response.status().as_u16() {
            200 | 201 => {}
            401 => return Err(PlatformError::Unauthorized),
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
    pub fn get(&self, path: &str, token: &str) -> Result<Value, PlatformError> {
        let response = self
            .agent
            .get(self.policy.github_url(path, true)?.as_str())
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "Nexora/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .call()
            .map_err(|_| PlatformError::Unavailable)?;
        Self::response(response)
    }
    fn token(&self, fields: &[(&str, &str)]) -> Result<GithubTokens, PlatformError> {
        let response = self
            .agent
            .post(
                self.policy
                    .github_url("/login/oauth/access_token", false)?
                    .as_str(),
            )
            .header("Accept", "application/json")
            .header("User-Agent", "Nexora/0.1")
            .send_form(fields.iter().copied())
            .map_err(|_| PlatformError::Unavailable)?;
        let value = Self::response(response)?;
        let result: GithubTokens =
            serde_json::from_value(value).map_err(|_| PlatformError::Unauthorized)?;
        result.validate()?;
        Ok(result)
    }
    pub fn refresh(
        &self,
        c: &GithubConfiguration,
        s: &GithubSecrets,
        refresh: &str,
    ) -> Result<GithubTokens, PlatformError> {
        self.token(&[
            ("client_id", &c.client_id),
            ("client_secret", &s.client_secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ])
    }
    pub fn id(value: &Value) -> Result<String, PlatformError> {
        value
            .as_u64()
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
            .ok_or(PlatformError::Unavailable)
    }
    pub fn app_permissions(value: &Value) -> Result<(), PlatformError> {
        let values = value.as_object().ok_or(PlatformError::Forbidden)?;
        if !matches!(
            values.get("contents").and_then(Value::as_str),
            Some("read" | "write")
        ) || values.get("metadata").and_then(Value::as_str) != Some("read")
            || values.iter().any(|(k, v)| match k.as_str() {
                "metadata" | "actions" | "checks" | "deployments" | "statuses" => v != "read",
                "administration" => v != "read" && v != "write",
                "contents" => v != "read" && v != "write",
                "pull_requests" => v != "read" && v != "write",
                _ => true,
            })
        {
            return Err(PlatformError::Forbidden);
        }
        Ok(())
    }
    /// Code fetch tokens must remain limited to the permissions explicitly requested.
    pub fn code_only_permissions(value: &Value) -> Result<(), PlatformError> {
        Self::app_permissions(value)?;
        if value["contents"] != "read"
            || value.as_object().is_none_or(|permissions| {
                permissions
                    .keys()
                    .any(|key| !["contents", "metadata"].contains(&key.as_str()))
            })
        {
            return Err(PlatformError::Forbidden);
        }
        Ok(())
    }
    pub fn user(&self, token: &str) -> Result<(String, String), PlatformError> {
        let v = self.get("/user", token)?;
        let login = v["login"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 160)
            .ok_or(PlatformError::Unauthorized)?;
        Ok((Self::id(&v["id"])?, login.into()))
    }
    pub fn installation_value(v: &Value, app: &str) -> Result<GithubInstallation, PlatformError> {
        if Self::id(&v["app_id"])? != app || !v["suspended_at"].is_null() {
            return Err(PlatformError::Forbidden);
        }
        Self::app_permissions(&v["permissions"])?;
        let login = v["account"]["login"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 160)
            .ok_or(PlatformError::Unavailable)?;
        let selection = v["repository_selection"]
            .as_str()
            .filter(|v| ["selected", "all"].contains(v))
            .ok_or(PlatformError::Unavailable)?;
        Ok(GithubInstallation {
            id: Self::id(&v["id"])?,
            account_id: Self::id(&v["account"]["id"])?,
            account_login: login.into(),
            repository_selection: selection.into(),
        })
    }
    pub fn installation(
        &self,
        config: &GithubConfiguration,
        secrets: &GithubSecrets,
        id: &str,
        user: &str,
    ) -> Result<GithubInstallation, PlatformError> {
        if id
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
            .as_deref()
            != Some(id)
        {
            return Err(PlatformError::Validation("invalid installation id"));
        }
        let v = self.get(
            &format!("/app/installations/{id}"),
            &GithubJwt::sign(&config.client_id, &secrets.private_key_pem)?,
        )?;
        let result = Self::installation_value(&v, &config.app_id)?;
        if result.id != id {
            return Err(PlatformError::Forbidden);
        }
        let proof = self.get(
            &format!("/user/installations/{id}/repositories?per_page=1"),
            user,
        )?;
        if !proof["repositories"].is_array() {
            return Err(PlatformError::Unavailable);
        }
        Ok(result)
    }
    pub fn repository(&self, token: &str, name: &str) -> Result<(String, String), PlatformError> {
        RepositoryRegistration {
            provider: GitProvider::Github,
            full_name: name.into(),
        }
        .locator()?;
        let v = self.get(&format!("/repos/{name}"), token)?;
        if v["disabled"].as_bool() != Some(false) {
            return Err(PlatformError::Forbidden);
        }
        let remote = v["full_name"].as_str().ok_or(PlatformError::Unavailable)?;
        RepositoryRegistration {
            provider: GitProvider::Github,
            full_name: remote.into(),
        }
        .locator()?;
        if !self
            .get(&format!("/repos/{remote}/branches?per_page=1"), token)?
            .is_array()
        {
            return Err(PlatformError::Unavailable);
        }
        Ok((Self::id(&v["id"])?, remote.into()))
    }
    pub fn repository_token(
        &self,
        config: &GithubConfiguration,
        secrets: &GithubSecrets,
        installation: &str,
        remote: &str,
    ) -> Result<GithubRepositoryToken, PlatformError> {
        let id = remote
            .parse::<u64>()
            .map_err(|_| PlatformError::Unavailable)?;
        let response = self
            .agent
            .post(
                self.policy
                    .github_url(
                        &format!("/app/installations/{installation}/access_tokens"),
                        true,
                    )?
                    .as_str(),
            )
            .header(
                "Authorization",
                format!(
                    "Bearer {}",
                    GithubJwt::sign(&config.client_id, &secrets.private_key_pem)?.as_str()
                ),
            )
            .header("User-Agent", "Nexora/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .send_json(
                json!({"repository_ids":[id],"permissions":{"contents":"read","metadata":"read"}}),
            )
            .map_err(|_| PlatformError::Unavailable)?;
        let v = Self::response(response)?;
        Self::code_only_permissions(&v["permissions"])?;
        let expiration = time::OffsetDateTime::parse(
            v["expires_at"].as_str().ok_or(PlatformError::Unavailable)?,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| PlatformError::Unavailable)?;
        let ttl = expiration.unix_timestamp() - time::OffsetDateTime::now_utc().unix_timestamp();
        if !(1..=3660).contains(&ttl) {
            return Err(PlatformError::Unauthorized);
        }
        let repos = v["repositories"]
            .as_array()
            .ok_or(PlatformError::Forbidden)?;
        if repos.len() != 1 || Self::id(&repos[0]["id"])? != remote {
            return Err(PlatformError::Forbidden);
        }
        let full_name = repos[0]["full_name"]
            .as_str()
            .ok_or(PlatformError::Unavailable)?;
        RepositoryRegistration {
            provider: GitProvider::Github,
            full_name: full_name.into(),
        }
        .locator()?;
        let token = v["token"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 16384 && !v.chars().any(char::is_control))
            .ok_or(PlatformError::Unauthorized)?;
        Ok(GithubRepositoryToken {
            access_token: Zeroizing::new(token.into()),
            full_name: full_name.into(),
        })
    }
}
