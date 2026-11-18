use crate::{
    EmailNotification, NotificationStore, email_token_cipher::EmailTokenCipher,
    master_key::MasterKey,
};
use nexora_platform::PlatformError;
use serde_json::Value;

impl NotificationStore {
    pub(crate) fn claim_action_email(
        &mut self,
        key: &str,
        kind: &str,
        resource: &str,
        actor: &str,
        attempts: i32,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let query = match kind {
            "email_verification" => {
                "SELECT q.email FROM user_email_changes q JOIN actors a ON a.id=q.actor_id WHERE q.id=$1 AND q.actor_id=$2 AND $3='email_verification:'||q.id AND q.used_at IS NULL AND q.expires_at>clock_timestamp() AND q.attempts<5 AND q.token_secret IS NOT NULL AND a.active AND COALESCE((SELECT email FROM notification_recipients WHERE actor_id=q.actor_id),'')=q.previous_email"
            }
            "email_changed" => {
                "SELECT q.previous_email FROM user_email_changes q JOIN actors a ON a.id=q.actor_id WHERE q.id=$1 AND q.actor_id=$2 AND $3='email_changed:'||q.id AND q.used_at IS NOT NULL AND q.previous_email<>'' AND a.active"
            }
            "invitation" => {
                "SELECT i.email FROM member_invitations i JOIN actors a ON a.id=i.invited_by WHERE i.id=$1 AND i.invited_by=$2 AND i.status='pending' AND i.expires_at>clock_timestamp() AND i.token_secret IS NOT NULL AND i.public_web_url=(SELECT public_web_url FROM email_access_settings WHERE singleton) AND $3='invitation:'||i.id||':'||i.revision AND a.active AND a.kind='human' AND (SELECT local_enabled FROM authentication_policy WHERE singleton) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='actors:manage' AND g.project_id IS NULL)"
            }
            "password_reset" => {
                "SELECT q.email FROM password_reset_requests q JOIN actors a ON a.id=q.actor_id JOIN users u ON u.actor_id=a.id JOIN notification_recipients r ON r.actor_id=a.id WHERE q.id=$1 AND q.actor_id=$2 AND $3='password_reset:'||q.id AND q.used_at IS NULL AND q.expires_at>clock_timestamp() AND q.token_secret IS NOT NULL AND a.active AND a.kind='human' AND u.password_hash IS NOT NULL AND lower(r.email)=q.email AND q.public_web_url=(SELECT public_web_url FROM email_access_settings WHERE singleton) AND (SELECT local_enabled FROM authentication_policy WHERE singleton)"
            }
            "invitation_accepted" => {
                "SELECT r.email FROM member_invitations i JOIN actors a ON a.id=i.invited_by JOIN notification_recipients r ON r.actor_id=a.id WHERE i.id=$1 AND i.invited_by=$2 AND $3='invitation_accepted:'||i.id AND i.status='accepted' AND r.email<>'' AND a.active AND a.kind='human' AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='actors:manage' AND g.project_id IS NULL)"
            }
            _ => return Err(PlatformError::Storage),
        };
        let row = self
            .client()
            .query_opt(query, &[&resource, &actor, &key])
            .map_err(Self::database_error)?;
        let Some(row) = row else {
            self.finish_email_notification(key, attempts, "skipped")?;
            return Ok(None);
        };
        let language = self
            .client()
            .query_one(
                "SELECT language FROM installation_preferences WHERE singleton",
                &[],
            )
            .map_err(Self::database_error)?
            .get(0);
        Ok(Some(EmailNotification {
            event_key: key.into(),
            kind: kind.into(),
            project_id: None,
            resource_id: resource.into(),
            actor_id: actor.into(),
            email: row.get(0),
            attempts,
            language,
        }))
    }

    pub(crate) fn claim_approval_result_email(
        &mut self,
        key: &str,
        resource: &str,
        actor: &str,
        attempts: i32,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let row = self.client().query_opt(
            "SELECT r.email,q.project_id,(SELECT language FROM installation_preferences WHERE singleton) FROM approval_requests q JOIN projects p ON p.id=q.project_id AND NOT p.archived JOIN actors a ON a.id=q.requested_by AND a.active AND a.kind='human' JOIN notification_recipients r ON r.actor_id=a.id WHERE q.id=$1 AND q.requested_by=$2 AND q.status IN ('approved','rejected') AND r.email<>'' AND r.approvals AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability=CASE WHEN q.target_kind='task' THEN 'tasks:read' ELSE 'documents:read' END AND (g.project_id IS NULL OR g.project_id=q.project_id))",
            &[&resource, &actor],
        ).map_err(Self::database_error)?;
        let Some(row) = row else {
            self.finish_email_notification(key, attempts, "skipped")?;
            return Ok(None);
        };
        Ok(Some(EmailNotification {
            event_key: key.into(),
            kind: "approval_result".into(),
            project_id: Some(row.get(1)),
            resource_id: resource.into(),
            actor_id: actor.into(),
            email: row.get(0),
            attempts,
            language: row.get(2),
        }))
    }

    pub(crate) fn email_action_token(
        &mut self,
        id: &str,
        revision: i64,
        envelope: Value,
    ) -> Result<zeroize::Zeroizing<String>, PlatformError> {
        let master = MasterKey::load(&MasterKey::path(&self.secrets_directory, &self.namespace))?;
        let state = self
            .client()
            .query_one(
                "SELECT key_version,key_id FROM platform_crypto_state WHERE singleton",
                &[],
            )
            .map_err(Self::database_error)?;
        if state.get::<_, i32>(0) != 1 || state.get::<_, String>(1) != master.id() {
            return Err(PlatformError::Conflict("installation master key mismatch"));
        }
        EmailTokenCipher::new(master, &self.namespace)?.open(id, revision, envelope)
    }
}
