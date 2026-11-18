pub(crate) struct ProviderSecretContext {
    pub binding_id: String,
    pub host: String,
    pub revision: i64,
}

impl ProviderSecretContext {
    pub(crate) fn smtp(settings: &serde_json::Value, revision: i64) -> Self {
        use sha2::{Digest, Sha256};

        let binding = serde_json::json!({
            "host": settings["host"],
            "port": settings["port"],
            "username": settings["username"],
            "tls_mode": settings["tls_mode"]
        });
        let host = format!("{:x}", Sha256::digest(binding.to_string().as_bytes()));
        Self {
            binding_id: "smtp-settings".into(),
            host,
            revision,
        }
    }
}
