use crate::RepositoryRuntimeScope;
use nexora_source::{GitFetchCredentials, GitObjectFormat};

/// Internal worker data; neither serializable nor printable, and contains no caller-selected scope.
pub struct RepositoryJobSourcePlan {
    pub(crate) origin: String,
    pub(crate) repository: String,
    pub(crate) commit: String,
    pub(crate) credentials: GitFetchCredentials,
    pub(crate) scope: RepositoryRuntimeScope,
}
impl RepositoryJobSourcePlan {
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub fn commit(&self) -> &str {
        &self.commit
    }
    pub fn credentials(&self) -> &GitFetchCredentials {
        &self.credentials
    }
    pub fn scope(&self) -> &RepositoryRuntimeScope {
        &self.scope
    }
    pub fn format(&self) -> GitObjectFormat {
        if self.commit.len() == 40 {
            GitObjectFormat::Sha1
        } else {
            GitObjectFormat::Sha256
        }
    }
}
