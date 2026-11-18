use nexora_git_policy::GitNetworkPolicy as SharedGitNetworkPolicy;
use nexora_platform::PlatformError;
use nexora_task_workflow::GitNetworkPolicy;
use std::path::Path;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn default_policy_uses_public_provider_origins_and_ip_loopback_callback() {
    let policy: SharedGitNetworkPolicy = GitNetworkPolicy::default();
    policy.validate().unwrap();

    assert_eq!(policy.gitlab_origin, "https://gitlab.com");
    assert_eq!(policy.callback_origin, "http://127.0.0.1:18080");
    assert_eq!(policy.github_origin, "https://github.com");
    assert_eq!(policy.github_api_origin, "https://api.github.com");
}

#[test]
fn existing_policy_files_default_omitted_github_origins() {
    let policy = GitNetworkPolicy::load(&fixture("legacy-policy.json")).unwrap();

    assert_eq!(policy.gitlab_origin, "https://gitlab.example.org");
    assert_eq!(policy.callback_origin, "https://nexora.example.org");
    assert_eq!(policy.github_origin, "https://github.com");
    assert_eq!(policy.github_api_origin, "https://api.github.com");
}

#[test]
fn rejects_malformed_policy_json() {
    assert!(matches!(
        GitNetworkPolicy::load(&fixture("malformed-policy.json")),
        Err(PlatformError::Validation("invalid Git policy"))
    ));
}

#[test]
fn rejects_unsafe_provider_and_callback_origins() {
    assert!(matches!(
        GitNetworkPolicy::load(&fixture("invalid-origin-policy.json")),
        Err(PlatformError::Validation(_))
    ));

    let invalid_origins = [
        GitNetworkPolicy {
            gitlab_origin: "http://gitlab.example.org".into(),
            ..GitNetworkPolicy::default()
        },
        GitNetworkPolicy {
            callback_origin: "http://localhost:18080".into(),
            ..GitNetworkPolicy::default()
        },
        GitNetworkPolicy {
            github_origin: "https://github.com/path".into(),
            ..GitNetworkPolicy::default()
        },
        GitNetworkPolicy {
            github_api_origin: "https://api.github.com?query=1".into(),
            ..GitNetworkPolicy::default()
        },
    ];

    for policy in invalid_origins {
        assert!(matches!(
            policy.validate(),
            Err(PlatformError::Validation(_))
        ));
    }
}
