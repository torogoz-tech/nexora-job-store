use crate::{
    AnalysisStore, GitNetworkPolicy, github_client::GithubClient,
    github_settings_store::GithubSettingsStore,
};
use nexora_platform::PlatformError;
use serde_json::json;

impl AnalysisStore {
    pub(crate) fn reconcile_github_webhook(
        &mut self,
        policy: &GitNetworkPolicy,
    ) -> Result<serde_json::Value, PlatformError> {
        let (configuration, secrets) = GithubSettingsStore::new(self).load(policy)?;
        let namespace = self.namespace.clone();
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                &[&format!("{}:github-settings", namespace)],
            )
            .map_err(Self::database_error)?;
        let revision: i64 = transaction
            .query_one(
                "SELECT revision FROM github_settings WHERE singleton AND enabled FOR SHARE",
                &[],
            )
            .map_err(Self::database_error)?
            .get(0);
        if configuration.revision != revision {
            return Err(PlatformError::Conflict(
                "GitHub configuration revision changed",
            ));
        }

        let url = format!("{}/api/v1/git/github/webhook", policy.callback_origin);
        let result = GithubClient::new(policy).configure_webhook(
            &configuration.client_id,
            &secrets.private_key_pem,
            &url,
            &secrets.webhook_secret,
        );
        let status = match &result {
            Ok(value) => value.clone(),
            Err(_) => json!({"url":url,"configuration_verified":false}),
        };
        transaction
            .execute(
                "INSERT INTO github_webhook_health(singleton,configuration_revision,status) VALUES(true,$1,$2) ON CONFLICT(singleton) DO UPDATE SET configuration_revision=$1,status=$2,checked_at=clock_timestamp()",
                &[&revision, &status],
            )
            .map_err(Self::database_error)?;
        transaction.commit().map_err(Self::database_error)?;
        result
    }
}
