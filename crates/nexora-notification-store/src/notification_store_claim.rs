use crate::{EmailNotification, NotificationStore};
use nexora_platform::PlatformError;

impl NotificationStore {
    pub fn claim_email_notification(
        &mut self,
        allow_business: bool,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let client = self.client();
        client
            .execute(
                "UPDATE user_email_changes SET token_secret=NULL WHERE expires_at<=clock_timestamp() OR used_at IS NOT NULL OR attempts>=5",
                &[],
            )
            .map_err(Self::database_error)?;
        client
            .execute(
                "UPDATE member_invitations SET token_secret=NULL WHERE expires_at<=clock_timestamp() OR status<>'pending'",
                &[],
            )
            .map_err(Self::database_error)?;
        client
            .execute(
                "UPDATE password_reset_requests SET token_secret=NULL WHERE expires_at<=clock_timestamp() OR used_at IS NOT NULL",
                &[],
            )
            .map_err(Self::database_error)?;
        client
            .execute(
                "UPDATE email_notifications SET status='failed' WHERE status='delivering' AND attempts>=5 AND available_at<=clock_timestamp()",
                &[],
            )
            .map_err(Self::database_error)?;
        let row = client
            .query_opt(
                "UPDATE email_notifications SET status='delivering',attempts=attempts+1,available_at=clock_timestamp()+interval '90 seconds' WHERE event_key=(SELECT event_key FROM email_notifications WHERE status IN ('pending','delivering') AND ($1 OR kind='test') AND attempts<5 AND available_at<=clock_timestamp() ORDER BY available_at,event_key LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING event_key,kind,project_id,resource_id,actor_id,attempts,recipient_override",
                &[&allow_business],
            )
            .map_err(Self::database_error)?;
        let Some(row) = row else {
            return Ok(None);
        };

        let actor: String = row.get(4);
        let kind: String = row.get(1);
        let project: Option<String> = row.get(2);
        let resource: String = row.get(3);
        let key: String = row.get(0);
        let attempts: i32 = row.get(5);

        if matches!(
            kind.as_str(),
            "invitation"
                | "invitation_accepted"
                | "password_reset"
                | "email_verification"
                | "email_changed"
        ) {
            return self.claim_action_email(&key, &kind, &resource, &actor, attempts);
        }
        if kind == "approval_result" {
            return self.claim_approval_result_email(&key, &resource, &actor, attempts);
        }
        if kind == "test" {
            return self.claim_test_email(row, key, kind, actor, resource, attempts);
        }
        if kind == "workflow_transition" {
            return self
                .claim_workflow_transition_email(key, kind, project, resource, actor, attempts);
        }

        self.claim_project_notification_email(key, kind, project, resource, actor, attempts)
    }

    fn claim_test_email(
        &mut self,
        row: postgres::Row,
        key: String,
        kind: String,
        actor: String,
        resource: String,
        attempts: i32,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let authorized = self
            .client()
            .query_opt(
                "SELECT a.id FROM actors a WHERE a.id=$1 AND a.active AND a.kind='human' AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='installation:manage' AND g.project_id IS NULL)",
                &[&actor],
            )
            .map_err(Self::database_error)?
            .is_some();
        if !authorized {
            self.finish_email_notification(&key, attempts, "skipped")?;
            return Ok(None);
        }

        let email: Option<String> = row.get(6);
        let language = self.notification_language()?;
        Ok(Some(EmailNotification {
            event_key: key,
            kind,
            project_id: None,
            resource_id: resource,
            actor_id: actor,
            email: email.ok_or(PlatformError::Storage)?,
            attempts,
            language,
        }))
    }

    fn claim_workflow_transition_email(
        &mut self,
        key: String,
        kind: String,
        project: Option<String>,
        resource: String,
        actor: String,
        attempts: i32,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let recipient = self
            .client()
            .query_opt(
                "SELECT r.email,(SELECT language FROM installation_preferences WHERE singleton) FROM notification_recipients r JOIN actors a ON a.id=r.actor_id AND a.active AND a.kind='human' JOIN tasks t ON t.id=$3 AND t.project_id IS NOT DISTINCT FROM $2::text AND t.assignee_id=$1 AND NOT t.archived WHERE r.actor_id=$1 AND r.email<>'' AND r.assignments AND ($2::text IS NULL OR EXISTS(SELECT 1 FROM projects p WHERE p.id=$2 AND NOT p.archived)) AND (EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=$1 AND g.capability='tasks:read' AND (g.project_id IS NULL OR g.project_id=$2)) OR ($2::text IS NULL AND EXISTS(SELECT 1 FROM actor_roles WHERE actor_id=$1 AND role IN ('super_admin','developer','technical_lead','pmo','manager'))))",
                &[&actor, &project, &resource],
            )
            .map_err(Self::database_error)?;
        let Some(recipient) = recipient else {
            self.finish_email_notification(&key, attempts, "skipped")?;
            return Ok(None);
        };

        Ok(Some(EmailNotification {
            event_key: key,
            kind,
            project_id: project,
            resource_id: resource,
            actor_id: actor,
            email: recipient.get(0),
            attempts,
            language: recipient.get(1),
        }))
    }

    fn claim_project_notification_email(
        &mut self,
        key: String,
        kind: String,
        project: Option<String>,
        resource: String,
        actor: String,
        attempts: i32,
    ) -> Result<Option<EmailNotification>, PlatformError> {
        let recipient=self.client().query_opt("SELECT r.email,(SELECT language FROM installation_preferences WHERE singleton) FROM notification_recipients r JOIN actors a ON a.id=r.actor_id AND a.active AND a.kind='human' JOIN projects p ON p.id=$2 AND NOT p.archived WHERE r.actor_id=$1 AND r.email<>'' AND CASE WHEN $3 IN ('assignment','workflow_transition') THEN r.assignments AND EXISTS(SELECT 1 FROM tasks t WHERE t.id=$4 AND t.project_id=$2 AND t.assignee_id=$1 AND NOT t.archived) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=$1 AND g.capability='tasks:read' AND (g.project_id IS NULL OR g.project_id=$2)) ELSE r.approvals AND EXISTS(SELECT 1 FROM approval_requests q WHERE q.id=$4 AND q.project_id=$2 AND q.status='pending' AND q.expires_at>clock_timestamp() AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=$1 AND g.capability=CASE WHEN q.target_kind='task' THEN 'tasks:read' ELSE 'documents:read' END AND (g.project_id IS NULL OR g.project_id=$2))) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=$1 AND g.capability='approvals:grant' AND (g.project_id IS NULL OR g.project_id=$2)) END",&[&actor,&project,&kind,&resource]).map_err(Self::database_error)?;
        let Some(recipient) = recipient else {
            self.finish_email_notification(&key, attempts, "skipped")?;
            return Ok(None);
        };

        Ok(Some(EmailNotification {
            event_key: key,
            kind,
            project_id: project,
            resource_id: resource,
            actor_id: actor,
            email: recipient.get(0),
            attempts,
            language: recipient.get(1),
        }))
    }

    fn notification_language(&mut self) -> Result<String, PlatformError> {
        self.client()
            .query_one(
                "SELECT language FROM installation_preferences WHERE singleton",
                &[],
            )
            .map(|row| row.get(0))
            .map_err(Self::database_error)
    }
}
