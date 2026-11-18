use crate::{ProviderSecretContext, SecretAuthority};
use nexora_platform::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(crate) struct InstallationServicePolicy;
impl InstallationServicePolicy {
    pub(crate) fn binding(kind: InstallationService, settings: &Value) -> String {
        let data = match kind {
            InstallationService::Openai => {
                json!({"endpoint":OpenAiEndpoint::normalize(settings["endpoint"].as_str().unwrap_or("")).unwrap_or_default()})
            }
            InstallationService::Smtp => {
                json!({"host":settings["host"],"port":settings["port"],"username":settings["username"],"tls_mode":settings["tls_mode"]})
            }
        };
        format!("{:x}", Sha256::digest(data.to_string().as_bytes()))
    }
    pub(crate) fn context(
        kind: InstallationService,
        settings: &Value,
        revision: i64,
    ) -> ProviderSecretContext {
        ProviderSecretContext {
            binding_id: format!("{}-settings", kind.as_str()),
            provider: match kind {
                InstallationService::Openai => SecretAuthority::OpenAi,
                InstallationService::Smtp => SecretAuthority::Smtp,
            },
            host: Self::binding(kind, settings),
            purpose: "installation_service_secret".into(),
            revision,
        }
    }
    pub fn validate(kind: InstallationService, value: &Value) -> Result<(), PlatformError> {
        match kind {
            InstallationService::Openai => {
                let s: OpenAiConfiguration = serde_json::from_value(value.clone())
                    .map_err(|_| PlatformError::Validation("invalid service configuration"))?;
                s.validate()
            }
            InstallationService::Smtp => {
                let s: SmtpConfiguration = serde_json::from_value(value.clone())
                    .map_err(|_| PlatformError::Validation("invalid service configuration"))?;
                s.validate()
            }
        }
    }
}
