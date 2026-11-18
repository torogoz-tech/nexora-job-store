use crate::NotificationStore;
use nexora_platform::PlatformError;

impl NotificationStore {
    pub fn finish_email_notification(
        &mut self,
        key: &str,
        attempts: i32,
        status: &str,
    ) -> Result<(), PlatformError> {
        if !matches!(status, "sent" | "skipped" | "pending") {
            return Err(PlatformError::Validation("invalid delivery result"));
        }
        let client = self.client();
        client.execute(
            "UPDATE email_notifications SET status=CASE WHEN $3='pending' AND attempts>=5 THEN 'failed' ELSE $3 END,delivered_at=CASE WHEN $3='sent' THEN clock_timestamp() ELSE delivered_at END,available_at=clock_timestamp()+LEAST(3600,30*power(2,attempts)::integer)*interval '1 second' WHERE event_key=$1 AND attempts=$2 AND status='delivering'",
            &[&key, &attempts, &status],
        ).map_err(Self::database_error)?;
        client.execute(
            "UPDATE member_invitations SET token_secret=NULL WHERE id IN(SELECT resource_id FROM email_notifications WHERE event_key=$1 AND kind='invitation' AND event_key='invitation:'||member_invitations.id||':'||member_invitations.revision AND status IN ('sent','skipped','failed'))",
            &[&key],
        ).map_err(Self::database_error)?;
        client.execute(
            "UPDATE password_reset_requests SET token_secret=NULL WHERE id IN(SELECT resource_id FROM email_notifications WHERE event_key=$1 AND kind='password_reset' AND status IN ('sent','skipped','failed'))",
            &[&key],
        ).map_err(Self::database_error)?;
        client.execute(
            "UPDATE user_email_changes SET token_secret=NULL WHERE id IN(SELECT resource_id FROM email_notifications WHERE event_key=$1 AND kind='email_verification' AND status IN ('sent','skipped','failed'))",
            &[&key],
        ).map_err(Self::database_error)?;
        Ok(())
    }
}
