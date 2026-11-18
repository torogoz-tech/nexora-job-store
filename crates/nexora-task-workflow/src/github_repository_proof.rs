use crate::{
    GitNetworkPolicy, TaskWorkflowStore, github_client::GithubClient,
    github_settings_store::GithubSettingsStore, github_token_store::GithubTokenStore,
};
use nexora_platform::*;
pub(crate) struct GithubRepositoryProof;
impl GithubRepositoryProof {
    pub(crate) fn check(
        store: &mut TaskWorkflowStore,
        connection: &GitConnection,
        name: &str,
        remote_id: Option<&str>,
        policy: &GitNetworkPolicy,
    ) -> Result<
        (
            String,
            crate::github_repository_token::GithubRepositoryToken,
        ),
        PlatformError,
    > {
        let tokens = GithubTokenStore::new(store).tokens(connection, policy)?;
        let (config, secrets) = GithubSettingsStore::new(store).load(policy)?;
        let client = GithubClient::new(policy);
        if client.user(&tokens.access_token)?.0 != connection.remote_account_id {
            return Err(PlatformError::Unauthorized);
        }
        let installation = connection
            .installation_id
            .as_deref()
            .ok_or(PlatformError::Unauthorized)?;
        // A denied/missing installation is not proof that the user revoked
        // this connection. Preserve its error category and fail this operation.
        client.installation(&config, &secrets, installation, &tokens.access_token)?;
        // New inscriptions resolve a name once; bound inscriptions follow their stable ID.
        let initial = if remote_id.is_none() {
            Some(client.repository(&tokens.access_token, name)?)
        } else {
            None
        };
        let id = remote_id
            .or_else(|| initial.as_ref().map(|(id, _)| id.as_str()))
            .ok_or(PlatformError::Storage)?;
        let scoped = client.repository_token(&config, &secrets, installation, id)?;
        let (user_id, user_name) = match initial {
            Some(ref value) if value.1 == scoped.full_name => value.clone(),
            _ => client.repository(&tokens.access_token, &scoped.full_name)?,
        };
        if user_id != id {
            return Err(PlatformError::Conflict(
                "registered repository identity cannot be replaced",
            ));
        }
        let (proven, remote) = client.repository(&scoped.access_token, &scoped.full_name)?;
        if proven != id {
            return Err(PlatformError::Unauthorized);
        }
        if remote != user_name {
            return Err(PlatformError::Conflict(
                "repository moved during provider verification",
            ));
        }
        Ok((id.into(), scoped))
    }
}
