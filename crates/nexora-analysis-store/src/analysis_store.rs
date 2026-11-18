use crate::{
    RepositoryJobLease, RepositoryJobSourcePlan, RepositoryRuntimeScope,
    analysis_profile_selection_store::AnalysisProfileSelectionStore, master_key::MasterKey,
    provider_secret_cipher::ProviderSecretCipher,
    repository_job_failure_store::RepositoryJobFailureStore,
    repository_job_lease_store::RepositoryJobLeaseStore,
    repository_job_publication_store::RepositoryJobPublicationStore,
    repository_job_source_plan_store::RepositoryJobSourcePlanStore,
};
use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_language::AnalysisProfilePackage;
use nexora_platform::{PlatformError, RepositoryBinding, RepositoryJob, RepositoryJobFailure};
use nexora_runtime::QuerySnapshot;
use postgres::{Client, error::SqlState};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;

pub struct AnalysisStore {
    connection: PostgresConnection,
    pub(crate) namespace: String,
    secret_directory: PathBuf,
}

impl AnalysisStore {
    pub fn connect(settings: &DatabaseConnections, namespace: &str) -> Result<Self, PlatformError> {
        settings
            .validate()
            .map_err(|_| PlatformError::Validation("invalid database configuration"))?;
        if namespace.is_empty()
            || namespace.len() > 63
            || !namespace.starts_with("nexora_platform")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PlatformError::Validation("invalid platform namespace"));
        }

        let mut connection =
            PostgresConnection::connect(settings).map_err(|_| PlatformError::Storage)?;
        let options = format!(
            "SET search_path TO {namespace},nexora_platform,pg_catalog; SET TIME ZONE 'UTC'; SET statement_timeout='15s'; SET lock_timeout='10s'"
        );
        connection
            .client()
            .batch_execute(&options)
            .map_err(Self::database_error)?;

        Ok(Self {
            connection,
            namespace: namespace.into(),
            secret_directory: settings.secrets_directory.clone(),
        })
    }

    pub fn claim_repository_job(
        &mut self,
        worker: &str,
        seconds: u16,
    ) -> Result<Option<RepositoryJobLease>, PlatformError> {
        RepositoryJobLeaseStore::new(self).claim(worker, seconds)
    }

    pub fn renew_repository_job(
        &mut self,
        lease: &RepositoryJobLease,
        seconds: u16,
    ) -> Result<RepositoryJobLease, PlatformError> {
        RepositoryJobLeaseStore::new(self).renew(lease, seconds)
    }

    pub fn fail_repository_job(
        &mut self,
        lease: &RepositoryJobLease,
        failure: RepositoryJobFailure,
    ) -> Result<RepositoryJob, PlatformError> {
        RepositoryJobFailureStore::record(self, lease, failure)
    }

    pub fn prepare_repository_job_source(
        &mut self,
        lease: &RepositoryJobLease,
        policy: &crate::GitNetworkPolicy,
    ) -> Result<RepositoryJobSourcePlan, PlatformError> {
        RepositoryJobSourcePlanStore::new(self).prepare(lease, policy)
    }

    pub fn pin_repository_job(
        &mut self,
        lease: &RepositoryJobLease,
        commit: &str,
    ) -> Result<RepositoryRuntimeScope, PlatformError> {
        RepositoryJobPublicationStore::new(self).pin(lease, commit)
    }

    pub fn complete_repository_job(
        &mut self,
        lease: &RepositoryJobLease,
        snapshot: &QuerySnapshot,
    ) -> Result<RepositoryBinding, PlatformError> {
        RepositoryJobPublicationStore::new(self).complete(lease, snapshot)
    }

    pub fn complete_repository_job_with_profile_selection(
        &mut self,
        lease: &RepositoryJobLease,
        snapshot: &QuerySnapshot,
        selection: &Value,
    ) -> Result<RepositoryBinding, PlatformError> {
        RepositoryJobPublicationStore::new(self).complete_with_selection(
            lease,
            snapshot,
            Some(selection),
        )
    }

    pub fn repository_job_analysis_profiles(
        &mut self,
        lease: &RepositoryJobLease,
    ) -> Result<Option<Value>, PlatformError> {
        AnalysisProfileSelectionStore::load(self, lease)
    }

    pub fn repository_job_analysis_profile(
        &mut self,
        lease: &RepositoryJobLease,
    ) -> Result<Option<AnalysisProfilePackage>, PlatformError> {
        let mut transaction = self
            .connection
            .client()
            .transaction()
            .map_err(Self::database_error)?;
        let row = crate::repository_job_execution_authority::RepositoryJobExecutionAuthority::lock(
            &mut transaction,
            lease,
            false,
        )?;
        let job: Value = row.get(0);
        let profile_id = job["analysis_profile_version_id"].as_str();
        let package = profile_id
            .map(|id| {
                let row = transaction
                    .query_opt(
                        "SELECT package,checksum FROM analysis_profile_versions WHERE id=$1",
                        &[&id],
                    )
                    .map_err(Self::database_error)?
                    .ok_or(PlatformError::NotFound)?;
                let package: AnalysisProfilePackage = Self::decode(row.get(0))?;
                let checksum: String = row.get(1);
                if package.checksum().map_err(|_| PlatformError::Storage)? != checksum {
                    return Err(PlatformError::Storage);
                }
                Ok(package)
            })
            .transpose()?;
        transaction.commit().map_err(Self::database_error)?;
        Ok(package)
    }

    pub(crate) fn client(&mut self) -> &mut Client {
        self.connection.client()
    }

    pub(crate) fn provider_secret_cipher(&mut self) -> Result<ProviderSecretCipher, PlatformError> {
        let key = MasterKey::load(&MasterKey::path(&self.secret_directory, &self.namespace))?;
        let state = self
            .connection
            .client()
            .query_one(
                "SELECT key_version,key_id FROM platform_crypto_state WHERE singleton",
                &[],
            )
            .map_err(Self::database_error)?;
        if state.get::<_, i32>(0) != 1 || state.get::<_, String>(1) != key.id() {
            return Err(PlatformError::Conflict("installation master key mismatch"));
        }
        Ok(ProviderSecretCipher::new(key, &self.namespace))
    }

    pub(crate) fn database_error(error: postgres::Error) -> PlatformError {
        match error.code() {
            Some(code) if code == &SqlState::UNIQUE_VIOLATION => {
                PlatformError::Conflict("resource already exists")
            }
            Some(code)
                if code == &SqlState::FOREIGN_KEY_VIOLATION
                    || code == &SqlState::CHECK_VIOLATION =>
            {
                PlatformError::Validation("invalid resource relation")
            }
            _ => PlatformError::Storage,
        }
    }

    pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, PlatformError> {
        serde_json::from_value(value).map_err(|_| PlatformError::Storage)
    }
}
