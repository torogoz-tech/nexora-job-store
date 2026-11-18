use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use nexora_core::NexoraError;
use nexora_storage::Snapshot;

pub(crate) struct SnapshotPayload;
impl SnapshotPayload {
    pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, NexoraError> {
        let mut writer = GzEncoder::new(Vec::new(), Compression::fast());
        serde_json::to_writer(&mut writer, snapshot)
            .map_err(|_| NexoraError::Storage("snapshot serialization failed".into()))?;
        writer.finish().map_err(NexoraError::from)
    }
    pub fn decode(bytes: &[u8]) -> Result<Snapshot, NexoraError> {
        // Existing uncompressed records remain readable without rewriting immutable snapshots.
        if bytes.starts_with(&[0x1f, 0x8b]) {
            serde_json::from_reader(std::io::BufReader::new(GzDecoder::new(bytes)))
        } else {
            serde_json::from_slice(bytes)
        }
        .map_err(|_| NexoraError::Storage("stored snapshot payload is invalid".into()))
    }
}
