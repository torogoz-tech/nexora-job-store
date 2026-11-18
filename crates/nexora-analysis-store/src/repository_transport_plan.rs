use nexora_source::{GitFetchCredentials, GitObjectFormat};

/// Verified provider transport needed by trusted repository workers.
pub struct RepositoryTransportPlan {
    origin: String,
    repository: String,
    base_commit: String,
    credentials: GitFetchCredentials,
}

impl RepositoryTransportPlan {
    pub(crate) fn new(
        origin: String,
        repository: String,
        base_commit: String,
        credentials: GitFetchCredentials,
    ) -> Self {
        Self {
            origin,
            repository,
            base_commit,
            credentials,
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn base_commit(&self) -> &str {
        &self.base_commit
    }

    pub fn credentials(&self) -> &GitFetchCredentials {
        &self.credentials
    }

    pub fn format(&self) -> GitObjectFormat {
        if self.base_commit.len() == 40 {
            GitObjectFormat::Sha1
        } else {
            GitObjectFormat::Sha256
        }
    }
}
