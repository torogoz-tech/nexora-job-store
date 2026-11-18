use sha2::{Digest, Sha256};
/// Binds source bytes to the immutable snapshot and exact evidence address.
pub(crate) struct SourceLineChecksum;
impl SourceLineChecksum {
    pub fn calculate(
        tenant: &str,
        repository: &str,
        snapshot: &str,
        path: &str,
        line: usize,
        bytes: &[u8],
    ) -> String {
        let mut digest = Sha256::new();
        digest.update(b"nexora-source-line-v1");
        for value in [tenant, repository, snapshot, path] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
        digest.update((line as u64).to_le_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }
}
