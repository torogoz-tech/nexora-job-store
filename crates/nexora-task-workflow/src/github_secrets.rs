use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Deserialize, Serialize)]
pub(crate) struct GithubSecrets {
    pub client_secret: String,
    pub private_key_pem: String,
    pub webhook_secret: String,
}
impl Drop for GithubSecrets {
    fn drop(&mut self) {
        self.client_secret.zeroize();
        self.private_key_pem.zeroize();
        self.webhook_secret.zeroize();
    }
}
