use nexora_platform::PlatformError;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub(crate) struct MasterKey {
    key: Zeroizing<[u8; 32]>,
}

impl MasterKey {
    pub(crate) fn path(directory: &Path, namespace: &str) -> PathBuf {
        directory.join(format!("{namespace}_master_key_v1"))
    }

    pub(crate) fn load(path: &Path) -> Result<Self, PlatformError> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| PlatformError::Storage)?;
        if !metadata.file_type().is_file() || metadata.len() != 32 {
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

        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&bytes);
        Ok(Self { key })
    }

    pub(crate) fn id(&self) -> String {
        Self::hex(&Sha256::digest(self.key.as_ref()))
    }

    pub(crate) fn provider_key(
        &self,
        namespace: &str,
    ) -> Result<Zeroizing<[u8; 32]>, PlatformError> {
        let hkdf = hkdf::Hkdf::<Sha256>::new(Some(b"nexora-platform-hkdf-v1"), self.key.as_ref());
        Self::derive_key(
            &hkdf,
            &format!("nexora-provider-secrets/aes256gcm/v1:{namespace}"),
        )
    }

    pub(crate) fn email_action_key(
        &self,
        namespace: &str,
    ) -> Result<Zeroizing<[u8; 32]>, PlatformError> {
        let hkdf =
            hkdf::Hkdf::<Sha256>::new(Some(b"nexora-email-action-hkdf-v1"), self.key.as_ref());
        Self::derive_key(
            &hkdf,
            &format!("nexora-email-action-aes256gcm-v1:{namespace}"),
        )
    }

    fn derive_key(
        hkdf: &hkdf::Hkdf<Sha256>,
        info: &str,
    ) -> Result<Zeroizing<[u8; 32]>, PlatformError> {
        let mut key = Zeroizing::new([0u8; 32]);
        hkdf.expand(info.as_bytes(), key.as_mut())
            .map_err(|_| PlatformError::Storage)?;
        Ok(key)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
