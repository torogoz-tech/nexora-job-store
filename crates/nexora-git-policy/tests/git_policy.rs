use nexora_git_policy::GitNetworkPolicy;
use nexora_platform::PlatformError;
use std::path::Path;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn defaults_match_the_existing_git_policy_contract() {
    let policy = GitNetworkPolicy::default();
    policy.validate().unwrap();

    assert_eq!(policy.gitlab_origin, "https://gitlab.com");
    assert_eq!(policy.callback_origin, "http://127.0.0.1:18080");
    assert_eq!(policy.github_origin, "https://github.com");
    assert_eq!(policy.github_api_origin, "https://api.github.com");
}

#[test]
fn loads_existing_json_with_omitted_github_origins() {
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
fn rejects_non_https_origins_except_ip_loopback_http() {
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

#[test]
fn serializes_the_existing_json_field_names_and_defaults() {
    let serialized = serde_json::to_value(GitNetworkPolicy::default()).unwrap();

    assert_eq!(serialized["gitlab_origin"], "https://gitlab.com");
    assert_eq!(serialized["callback_origin"], "http://127.0.0.1:18080");
    assert_eq!(serialized["github_origin"], "https://github.com");
    assert_eq!(serialized["github_api_origin"], "https://api.github.com");
}

#[test]
fn rejects_unknown_policy_fields() {
    let input = r#"{
        "gitlab_origin": "https://gitlab.example.org",
        "callback_origin": "https://nexora.example.org",
        "unknown_origin": "https://attacker.example.org"
    }"#;

    assert!(serde_json::from_str::<GitNetworkPolicy>(input).is_err());
}
