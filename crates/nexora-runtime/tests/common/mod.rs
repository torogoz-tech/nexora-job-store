use std::path::PathBuf;

pub fn connections_config_path() -> PathBuf {
    required_path("NEXORA_TEST_CONNECTIONS")
}

pub fn javascript_profile_path() -> PathBuf {
    required_path("NEXORA_TEST_LANGUAGE_PROFILE")
}

#[allow(dead_code)]
pub fn domain_fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/domain-boundary")
        .join(name)
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name} to run Nexora runtime integration tests"))
}
