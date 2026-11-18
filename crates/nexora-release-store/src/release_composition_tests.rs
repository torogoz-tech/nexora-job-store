use crate::release_composition::ReleaseComposition;
use nexora_platform::PlatformError;

#[test]
fn only_requests_creation_when_the_release_branch_does_not_exist() {
    assert!(ReleaseComposition::branch_needs_push(None, "abc123").unwrap());
    assert!(!ReleaseComposition::branch_needs_push(Some("abc123"), "abc123").unwrap());
}

#[test]
fn refuses_to_overwrite_a_release_branch_with_another_commit() {
    assert!(matches!(
        ReleaseComposition::branch_needs_push(Some("existing"), "expected"),
        Err(PlatformError::Conflict(_))
    ));
}

#[test]
fn requires_between_one_and_one_hundred_explicit_commits() {
    assert!(ReleaseComposition::validate_commit_count(1).is_ok());
    assert!(ReleaseComposition::validate_commit_count(100).is_ok());
    assert!(ReleaseComposition::validate_commit_count(0).is_err());
    assert!(ReleaseComposition::validate_commit_count(101).is_err());
}
