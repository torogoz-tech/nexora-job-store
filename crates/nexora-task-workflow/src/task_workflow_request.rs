use crate::{
    TaskWorkflowStore, master_key::MasterKey,
    task_workflow_request_context::TaskWorkflowRequestContext,
};
use nexora_platform::{AuthContext, PlatformError};
use serde::Serialize;

impl TaskWorkflowStore {
    /// Runs a task-workflow mutation with the platform's durable idempotency receipt.
    pub fn request<T, F>(
        &mut self,
        context: &AuthContext,
        operation: &str,
        request_id: &str,
        input: &impl Serialize,
        action: F,
    ) -> Result<T, PlatformError>
    where
        F: FnOnce(&mut Self) -> Result<T, PlatformError>,
    {
        if self.request_context.is_some() {
            return Err(PlatformError::Conflict("nested mutation request"));
        }
        if request_id.is_empty()
            || request_id.len() > 128
            || !request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
            || request_id.starts_with("nxk_")
            || request_id.starts_with("nxs_")
        {
            return Err(PlatformError::Validation("invalid idempotency key"));
        }
        if operation.is_empty() || operation.len() > 128 {
            return Err(PlatformError::Validation("invalid mutation operation"));
        }
        let input = serde_json::to_value(input).map_err(|_| PlatformError::Storage)?;
        let bytes = serde_json::to_vec(&(
            &self.namespace,
            context.actor_id(),
            operation,
            request_id,
            input,
        ))
        .map_err(|_| PlatformError::Storage)?;
        if bytes.len() > 1_048_576 {
            return Err(PlatformError::Validation("request exceeds limit"));
        }
        let key = self.installation_master_key()?;
        let fingerprint = key.fingerprint(&bytes)?;
        self.request_context = Some(TaskWorkflowRequestContext {
            actor_id: context.actor_id().into(),
            operation: operation.into(),
            request_id: request_id.into(),
            fingerprint,
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(self)));
        self.request_context = None;
        match result {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    pub(crate) fn installation_master_key(&mut self) -> Result<MasterKey, PlatformError> {
        let key = MasterKey::load(&MasterKey::path(&self.secret_directory, &self.namespace))?;
        let state = self
            .client()
            .query_one(
                "SELECT key_version,key_id FROM platform_crypto_state WHERE singleton",
                &[],
            )
            .map_err(Self::database_error)?;
        if state.get::<_, i32>(0) != 1 || state.get::<_, String>(1) != key.id() {
            return Err(PlatformError::Conflict("installation master key mismatch"));
        }
        Ok(key)
    }
}
