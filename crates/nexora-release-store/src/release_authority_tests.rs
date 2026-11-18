use crate::release_authority::ReleaseAuthority;
use nexora_platform::{
    ActorKind, AuthContext, AuthMethod, Capability, CapabilityGrant, KeyScope, PlatformError,
};

fn context(scope: Option<KeyScope>, grants: Vec<CapabilityGrant>) -> AuthContext {
    AuthContext::new(
        "act_worker_test".into(),
        ActorKind::Human,
        AuthMethod::ApiKey,
        "key_test".into(),
        grants,
        scope,
    )
}

#[test]
fn rejects_a_context_without_the_required_project_capability() {
    let context = context(
        Some(KeyScope {
            capabilities: vec![Capability::TasksRead],
            project_ids: vec!["prj_test".into()],
            all_projects: false,
            installation: false,
        }),
        vec![CapabilityGrant::new(
            Capability::TasksRead,
            Some("prj_test".into()),
        )],
    );

    assert!(matches!(
        ReleaseAuthority::require_context(&context, Capability::TasksWrite, "prj_test"),
        Err(PlatformError::Forbidden)
    ));
}

#[test]
fn rejects_a_context_scoped_to_another_project() {
    let context = context(
        Some(KeyScope {
            capabilities: vec![Capability::TasksWrite],
            project_ids: vec!["prj_other".into()],
            all_projects: false,
            installation: false,
        }),
        vec![CapabilityGrant::new(
            Capability::TasksWrite,
            Some("prj_other".into()),
        )],
    );

    assert!(matches!(
        ReleaseAuthority::require_context(&context, Capability::TasksWrite, "prj_test"),
        Err(PlatformError::Forbidden)
    ));
}

#[test]
fn rejects_project_scoped_keys_for_installation_level_authority() {
    let context = context(
        Some(KeyScope {
            capabilities: vec![Capability::InstallationManage],
            project_ids: vec!["prj_test".into()],
            all_projects: false,
            installation: false,
        }),
        vec![CapabilityGrant::new(
            Capability::InstallationManage,
            Some("prj_test".into()),
        )],
    );

    assert!(matches!(
        ReleaseAuthority::require_installation_context(&context, Capability::InstallationManage),
        Err(PlatformError::Forbidden)
    ));
}
