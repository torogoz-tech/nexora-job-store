use super::GitlabTokens;
#[test]
fn accepts_explicit_governed_api_consent_and_preserves_old_read_only_connections() {
    for (scope, valid) in [
        ("api", true),
        ("read_api read_repository", true),
        ("api read_repository", true),
        ("read_api", false),
        ("api sudo", false),
        ("write_repository", false),
    ] {
        let token = GitlabTokens {
            access_token: "fixture-token".into(),
            refresh_token: None,
            token_type: "Bearer".into(),
            scope: scope.into(),
            expires_in: 3600,
        };
        assert_eq!(token.validate().is_ok(), valid, "{scope}");
    }
}
