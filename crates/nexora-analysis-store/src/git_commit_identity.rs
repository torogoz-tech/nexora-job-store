use nexora_platform::PlatformError;
use serde_json::Value;

pub(crate) struct GitCommitIdentity;
impl GitCommitIdentity {
    pub fn valid(value: &str) -> bool {
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    pub fn response(
        value: &Value,
        field: &str,
        pinned: Option<&str>,
    ) -> Result<String, PlatformError> {
        let object = if pinned.is_some() {
            value
        } else {
            let list = value.as_array().ok_or(PlatformError::Unavailable)?;
            if list.is_empty() {
                return Err(PlatformError::Conflict("repository has no commit"));
            }
            if list.len() != 1 {
                return Err(PlatformError::Unavailable);
            }
            &list[0]
        };
        let oid = object[field]
            .as_str()
            .filter(|id| Self::valid(id))
            .ok_or(PlatformError::Unavailable)?;
        if pinned.is_some_and(|expected| expected != oid) {
            return Err(PlatformError::Conflict(
                "provider returned a different pinned commit",
            ));
        }
        Ok(oid.into())
    }
}
