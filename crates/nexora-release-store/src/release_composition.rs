use nexora_platform::PlatformError;

pub(crate) struct ReleaseComposition;

impl ReleaseComposition {
    pub(crate) fn branch_needs_push(
        existing_head: Option<&str>,
        expected_head: &str,
    ) -> Result<bool, PlatformError> {
        match existing_head {
            None => Ok(true),
            Some(existing) if existing == expected_head => Ok(false),
            Some(_) => Err(PlatformError::Conflict(
                "release branch already exists with another commit; it will not be overwritten",
            )),
        }
    }

    pub(crate) fn validate_commit_count(count: usize) -> Result<(), PlatformError> {
        if (1..=100).contains(&count) {
            Ok(())
        } else {
            Err(PlatformError::Validation(
                "release composition requires between 1 and 100 explicit commits per repository",
            ))
        }
    }
}
