use nexora_platform::PlatformError;
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
use zeroize::Zeroizing;

pub(crate) struct KnowledgeStoreMasterKey {
    key: Zeroizing<[u8; 32]>,
}

impl KnowledgeStoreMasterKey {
    pub(crate) fn load(directory: &Path, namespace: &str) -> Result<Self, PlatformError> {
        let path = directory.join(format!("{namespace}_master_key_v1"));
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| PlatformError::Storage)?;
        if !metadata.is_file() || metadata.len() != 32 {
            return Err(PlatformError::Storage);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(PlatformError::Storage);
            }
        }

        let mut bytes = Zeroizing::new(Vec::new());
        std::fs::File::open(path)
            .map_err(|_| PlatformError::Storage)?
            .take(33)
            .read_to_end(&mut bytes)
            .map_err(|_| PlatformError::Storage)?;
        if bytes.len() != 32 {
            return Err(PlatformError::Storage);
        }

        let mut key = Zeroizing::new([0_u8; 32]);
        key.copy_from_slice(&bytes);
        Ok(Self { key })
    }

    pub(crate) fn id(&self) -> String {
        Sha256::digest(self.key.as_ref())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.key
    }
}
