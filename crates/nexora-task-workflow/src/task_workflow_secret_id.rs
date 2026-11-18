use nexora_platform::PlatformError;

/// Generates non-secret identifiers using operating-system entropy.
pub(crate) struct TaskWorkflowSecretId;

impl TaskWorkflowSecretId {
    pub(crate) fn generate(prefix: &str) -> Result<String, PlatformError> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| PlatformError::Unavailable)?;
        let suffix = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("{prefix}_{suffix}"))
    }
}
