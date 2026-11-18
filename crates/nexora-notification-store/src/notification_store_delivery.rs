use crate::{EmailNotification, NotificationStore};
use nexora_platform::{EmailContentTemplate, EmailTemplate, PlatformError};
use serde_json::Value;
use std::collections::BTreeMap;

impl NotificationStore {
    pub fn email_template_for_delivery(
        &mut self,
    ) -> Result<(EmailTemplate, String), PlatformError> {
        let row = self.client().query_one(
            "SELECT t.html,t.revision,p.name FROM installation_email_template t CROSS JOIN organization_profile p WHERE t.singleton AND p.singleton",
            &[],
        ).map_err(Self::database_error)?;
        Ok((
            EmailTemplate {
                html: row
                    .get::<_, Option<String>>(0)
                    .unwrap_or_else(|| EmailTemplate::DEFAULT_HTML.to_owned()),
                revision: row.get(1),
            },
            row.get(2),
        ))
    }

    pub fn email_content_for_delivery(
        &mut self,
        mail: &EmailNotification,
        organization: &str,
    ) -> Result<(String, String), PlatformError> {
        let row = self.client().query_one(
            "SELECT subject,body_html FROM installation_email_contents WHERE kind=$1 AND language=$2",
            &[&mail.kind, &mail.language],
        ).map_err(Self::database_error)?;
        let mut item = EmailContentTemplate::definition(&mail.kind, &mail.language)?;
        item.subject = row.get(0);
        item.body_html = row.get(1);
        let recipient_name: String = self
            .client()
            .query_one("SELECT name FROM actors WHERE id=$1", &[&mail.actor_id])
            .map_err(Self::database_error)?
            .get(0);
        let mut values = BTreeMap::from([
            ("organization_name".into(), organization.into()),
            ("recipient_name".into(), recipient_name),
        ]);
        if let Some(project_id) = &mail.project_id {
            let project_name: String = self
                .client()
                .query_one("SELECT name FROM projects WHERE id=$1", &[project_id])
                .map_err(Self::database_error)?
                .get(0);
            values.insert("project_name".into(), project_name);
        }
        values.insert("resource_id".into(), mail.resource_id.clone());
        self.email_action_parameters(mail, &mut values)?;
        item.render(&values)
    }

    fn email_action_parameters(
        &mut self,
        mail: &EmailNotification,
        values: &mut BTreeMap<String, String>,
    ) -> Result<(), PlatformError> {
        match mail.kind.as_str() {
            "email_verification" => {
                let secret: Value = self.client().query_one(
                    "SELECT token_secret FROM user_email_changes WHERE id=$1 AND actor_id=$2 AND used_at IS NULL AND expires_at>clock_timestamp() AND attempts<5",
                    &[&mail.resource_id, &mail.actor_id],
                ).map_err(Self::database_error)?.get::<_, Option<Value>>(0).ok_or(PlatformError::Storage)?;
                let code = self.email_action_token(&mail.resource_id, 1, secret)?;
                values.insert("verification_code".into(), code.as_str().into());
            }
            "email_changed" => {
                let address: String = self.client().query_one(
                    "SELECT email FROM user_email_changes WHERE id=$1 AND actor_id=$2 AND used_at IS NOT NULL",
                    &[&mail.resource_id, &mail.actor_id],
                ).map_err(Self::database_error)?.get(0);
                values.insert("new_email".into(), address);
            }
            "invitation" => {
                let row = self.client().query_one(
                    "SELECT i.name,a.name,i.public_web_url,i.expires_at::text,i.token_secret,i.revision FROM member_invitations i JOIN actors a ON a.id=i.invited_by WHERE i.id=$1 AND i.status='pending' AND i.expires_at>clock_timestamp() AND $2='invitation:'||i.id||':'||i.revision",
                    &[&mail.resource_id, &mail.event_key],
                ).map_err(Self::database_error)?;
                let envelope: Value = row
                    .get::<_, Option<Value>>(4)
                    .ok_or(PlatformError::Storage)?;
                let token = self.email_action_token(&mail.resource_id, row.get(5), envelope)?;
                let web: String = row.get(2);
                values.insert("recipient_name".into(), row.get(0));
                values.insert("inviter_name".into(), row.get(1));
                values.insert("expires_at".into(), row.get(3));
                values.insert(
                    "invitation_url".into(),
                    format!("{web}/invitations/accept#token={}", token.as_str()),
                );
            }
            "password_reset" => {
                let row = self.client().query_one(
                    "SELECT public_web_url,expires_at::text,token_secret FROM password_reset_requests WHERE id=$1 AND used_at IS NULL AND expires_at>clock_timestamp()",
                    &[&mail.resource_id],
                ).map_err(Self::database_error)?;
                let envelope: Value = row
                    .get::<_, Option<Value>>(2)
                    .ok_or(PlatformError::Storage)?;
                let token = self.email_action_token(&mail.resource_id, 1, envelope)?;
                let web: String = row.get(0);
                values.insert("expires_at".into(), row.get(1));
                values.insert(
                    "reset_url".into(),
                    format!("{web}/reset-password#token={}", token.as_str()),
                );
            }
            "invitation_accepted" => {
                let name: String = self.client().query_one(
                    "SELECT a.name FROM member_invitations i JOIN actors a ON a.id=i.accepted_by WHERE i.id=$1 AND i.status='accepted'",
                    &[&mail.resource_id],
                ).map_err(Self::database_error)?.get(0);
                values.insert("member_name".into(), name);
            }
            "approval_result" => {
                let row = self.client().query_one(
                    "SELECT status,COALESCE(decision_reason,'') FROM approval_requests WHERE id=$1 AND status IN ('approved','rejected')",
                    &[&mail.resource_id],
                ).map_err(Self::database_error)?;
                let status: String = row.get(0);
                let decision = match (status.as_str(), mail.language.as_str()) {
                    ("approved", "es") => "Aprobada",
                    ("rejected", "es") => "Rechazada",
                    ("approved", _) => "Approved",
                    _ => "Rejected",
                };
                values.insert("decision".into(), decision.into());
                values.insert("reason".into(), row.get(1));
            }
            _ => {}
        }
        Ok(())
    }
}
