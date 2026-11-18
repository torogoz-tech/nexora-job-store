use nexora_core::NexoraError;
use serde::Serialize;
use std::io::Write;
pub(crate) struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
}
impl BoundedJson {
    pub fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, NexoraError> {
        let mut target = Self {
            bytes: Vec::new(),
            limit,
        };
        serde_json::to_writer(&mut target, value).map_err(|_| {
            NexoraError::Limit("serialized database payload exceeds limit or is invalid".into())
        })?;
        Ok(target.bytes)
    }
}
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("JSON limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
