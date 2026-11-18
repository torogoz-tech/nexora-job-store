use std::path::Path;

pub(crate) fn write_master_key(directory: &Path, namespace: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!("{namespace}_master_key_v1"));
    std::fs::write(&path, [7u8; 32])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
