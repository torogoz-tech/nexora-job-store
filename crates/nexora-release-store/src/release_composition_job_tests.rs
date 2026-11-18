use crate::release_composition_job::ReleaseCompositionJob;
use nexora_platform::{AuthMethod, PlatformError};

#[test]
fn parses_persisted_session_and_api_key_credentials() {
    assert!(matches!(
        ReleaseCompositionJob::auth_method("session"),
        Ok(AuthMethod::Session)
    ));
    assert!(matches!(
        ReleaseCompositionJob::auth_method("api_key"),
        Ok(AuthMethod::ApiKey)
    ));
}

#[test]
fn rejects_unknown_persisted_authentication_methods() {
    assert!(matches!(
        ReleaseCompositionJob::auth_method("system"),
        Err(PlatformError::Storage)
    ));
}
