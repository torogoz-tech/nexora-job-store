use nexora_analysis_store::GitNetworkPolicy as AnalysisGitNetworkPolicy;
use nexora_git_policy::GitNetworkPolicy as SharedGitNetworkPolicy;

fn accepts_shared_policy_type(_: &SharedGitNetworkPolicy) {}

#[test]
fn public_analysis_path_reexports_the_shared_policy_type() {
    let shared = SharedGitNetworkPolicy::default();
    accepts_shared_policy_type(&shared);

    let through_analysis_path: AnalysisGitNetworkPolicy = shared;
    assert_eq!(through_analysis_path.github_origin, "https://github.com");
}
