use crate::{
    GitNetworkPolicy, gitlab_client::GitlabClient,
    repository_transport_plan_store::RepositoryTransportPlanStore,
};
use nexora_platform::{GitConnection, ProjectRepository};

fn connection() -> GitConnection {
    serde_json::from_value(serde_json::json!({
        "id": "git_connection_1",
        "provider": "github",
        "host": "github.com",
        "owner_actor_id": "act_1",
        "remote_account_id": "123",
        "remote_login": "developer",
        "installation_id": "456",
        "status": "active",
        "revision": 1,
        "expires_at": "2027-01-01T00:00:00Z",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

fn repository() -> ProjectRepository {
    serde_json::from_value(serde_json::json!({
        "id": "repo_1",
        "project_id": "prj_1",
        "provider": "github",
        "host": "github.com",
        "requested_full_name": "torogoz-tech/example",
        "remote_id": "789",
        "remote_full_name": "torogoz-tech/example",
        "connection_id": "git_connection_1",
        "authorization_status": "authorized",
        "archived": false,
        "revision": 1,
        "source_generation": 1,
        "latest_source_commit": null,
        "source_changed_at": null,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn accepts_only_an_active_authorized_git_repository_bound_to_the_connection() {
    assert!(
        RepositoryTransportPlanStore::validate_source(
            &connection(),
            &repository(),
            &GitNetworkPolicy::default(),
        )
        .is_ok()
    );
}

#[test]
fn rejects_a_repository_bound_to_another_git_connection() {
    let mut repository = repository();
    repository.connection_id = Some("git_connection_2".into());

    assert!(
        RepositoryTransportPlanStore::validate_source(
            &connection(),
            &repository,
            &GitNetworkPolicy::default(),
        )
        .is_err()
    );
}

#[test]
fn rejects_a_repository_whose_provider_host_does_not_match_the_connection_policy() {
    let mut repository = repository();
    repository.host = Some("attacker.example".into());

    assert!(
        RepositoryTransportPlanStore::validate_source(
            &connection(),
            &repository,
            &GitNetworkPolicy::default(),
        )
        .is_err()
    );
}

#[test]
fn rejects_invalid_gitlab_branch_names_before_network_access() {
    let client = GitlabClient::new(&GitNetworkPolicy::default());

    assert!(client.branch("test-token", "123", "../main").is_err());
}
