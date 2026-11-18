use crate::SecretAuthority;
/// Metadata identifying a provider-secret record. Does not authenticate its owner.
#[derive(Clone)]
pub struct ProviderSecretContext {
    pub binding_id: String,
    pub provider: SecretAuthority,
    pub host: String,
    pub purpose: String,
    pub revision: i64,
}
