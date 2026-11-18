use nexora_core::NexoraError;
use nexora_storage::Snapshot;
use sha2::{Digest, Sha256};
use std::io::{BufWriter, Result, Write};

pub(crate) struct SnapshotChecksum(Sha256);
impl SnapshotChecksum {
    pub fn calculate(snapshot: &Snapshot) -> std::result::Result<String, NexoraError> {
        let mut writer = Self(Sha256::new());
        {
            let mut buffer = BufWriter::with_capacity(65_536, &mut writer);
            serde_json::to_writer(&mut buffer, snapshot)
                .map_err(|_| NexoraError::Storage("snapshot serialization failed".into()))?;
            buffer.flush().map_err(NexoraError::from)?;
        }
        Ok(format!("{:x}", writer.0.finalize()))
    }
}
impl Write for SnapshotChecksum {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
