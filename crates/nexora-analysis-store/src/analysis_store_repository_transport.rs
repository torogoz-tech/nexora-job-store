use crate::{
    AnalysisStore, GitNetworkPolicy, RepositoryTransportPlan,
    repository_transport_plan_store::RepositoryTransportPlanStore,
};
use nexora_platform::{GitConnection, PlatformError, ProjectRepository};

impl AnalysisStore {
    /// Creates a provider transport after the caller authorizes its durable job.
    pub fn prepare_repository_transport_plan(
        &mut self,
        connection: &GitConnection,
        repository: &ProjectRepository,
        policy: &GitNetworkPolicy,
        branch: &str,
    ) -> Result<RepositoryTransportPlan, PlatformError> {
        RepositoryTransportPlanStore::new(self).prepare(connection, repository, policy, branch)
    }
}
