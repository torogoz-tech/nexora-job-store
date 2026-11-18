use nexora_platform::PlatformError;
use serde_json::Value;
use zeroize::Zeroizing;

/// Installation SMTP configuration and its decrypted credential for one delivery attempt.
pub struct NotificationDeliveryRuntime {
    pub settings: Value,
    pub secret: Option<Zeroizing<Vec<u8>>>,
}

impl NotificationDeliveryRuntime {
    pub fn smtp_configuration(&self) -> Result<nexora_platform::SmtpConfiguration, PlatformError> {
        let configuration: nexora_platform::SmtpConfiguration =
            serde_json::from_value(self.settings.clone()).map_err(|_| PlatformError::Storage)?;
        configuration.validate()?;
        Ok(configuration)
    }
}
