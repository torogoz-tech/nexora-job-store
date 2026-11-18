use crate::{KnowledgeProcessingJob, knowledge_store_secret::KnowledgeStoreSecret};
use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_platform::PlatformError;
use postgres::{Client, error::SqlState};

pub struct KnowledgeStore {
    connection: PostgresConnection,
}

impl KnowledgeStore {
    pub fn connect(settings: &DatabaseConnections, namespace: &str) -> Result<Self, PlatformError> {
        if namespace.is_empty()
            || namespace.len() > 63
            || !namespace.starts_with("nexora_")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PlatformError::Validation("invalid platform namespace"));
        }

        let mut connection =
            PostgresConnection::connect(settings).map_err(|_| PlatformError::Storage)?;
        let options = format!(
            "SET search_path TO {namespace},nexora_platform,pg_catalog; SET TIME ZONE 'UTC'; SET statement_timeout='15s'; SET lock_timeout='10s'"
        );
        connection
            .client()
            .batch_execute(&options)
            .map_err(Self::database_error)?;

        Ok(Self { connection })
    }

    pub fn claim_knowledge_processing(
        &mut self,
    ) -> Result<Option<KnowledgeProcessingJob>, PlatformError> {
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        transaction
            .execute(
                "UPDATE knowledge_processing_jobs SET status='failed',last_error='processing attempts exhausted',updated_at=clock_timestamp() WHERE status='processing' AND lease_until<=clock_timestamp() AND attempts>=3",
                &[],
            )
            .map_err(Self::database_error)?;
        let row = transaction
            .query_opt(
                "SELECT id,project_id,source_kind,resource_id,source_revision,content_hash FROM knowledge_processing_jobs j WHERE ((status='queued') OR (status='processing' AND lease_until<=clock_timestamp())) AND attempts<3 AND EXISTS(SELECT 1 FROM knowledge_reviews r WHERE r.resource_id=j.resource_id AND r.resource_revision=j.source_revision AND r.status='approved' AND ((r.kind='contribution' AND j.source_kind IN ('document','technical_debt')) OR r.kind=j.source_kind)) ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1",
                &[],
            )
            .map_err(Self::database_error)?;
        let Some(row) = row else {
            transaction.commit().map_err(Self::database_error)?;
            return Ok(None);
        };

        let job_id: String = row.get(0);
        let lease_token = KnowledgeStoreSecret::token("nkr")?;
        let lease_digest = KnowledgeStoreSecret::digest(&lease_token);
        transaction
            .execute(
                "UPDATE knowledge_processing_jobs SET status='processing',attempts=attempts+1,claimed_at=clock_timestamp(),lease_owner='knowledge-synthesis-worker',lease_token_digest=$2,lease_until=clock_timestamp()+interval '15 minutes',updated_at=clock_timestamp() WHERE id=$1",
                &[&job_id, &lease_digest],
            )
            .map_err(Self::database_error)?;
        let job = KnowledgeProcessingJob {
            id: job_id,
            project_id: row.get(1),
            source_kind: row.get(2),
            resource_id: row.get(3),
            source_revision: row.get(4),
            content_hash: row.get(5),
            lease_token,
        };
        transaction.commit().map_err(Self::database_error)?;
        Ok(Some(job))
    }

    pub fn fail_knowledge_processing(
        &mut self,
        job: &KnowledgeProcessingJob,
        error: &str,
    ) -> Result<bool, PlatformError> {
        let lease_digest = KnowledgeStoreSecret::digest(&job.lease_token);
        let safe_error = error.chars().take(500).collect::<String>();
        let changed = self
            .client()
            .execute(
                "UPDATE knowledge_processing_jobs SET status='failed',last_error=$2,lease_owner=NULL,lease_token_digest=NULL,lease_until=NULL,updated_at=clock_timestamp() WHERE id=$1 AND status IN ('queued','processing','structured') AND lease_token_digest=$3 AND lease_until>clock_timestamp()",
                &[&job.id, &safe_error, &lease_digest],
            )
            .map_err(Self::database_error)?;
        Ok(changed == 1)
    }

    pub(crate) fn client(&mut self) -> &mut Client {
        self.connection.client()
    }

    pub(crate) fn database_error(error: postgres::Error) -> PlatformError {
        match error.code() {
            Some(code) if code == &SqlState::UNIQUE_VIOLATION => {
                PlatformError::Conflict("resource already exists")
            }
            Some(code)
                if code == &SqlState::FOREIGN_KEY_VIOLATION
                    || code == &SqlState::CHECK_VIOLATION =>
            {
                PlatformError::Validation("invalid resource relation")
            }
            _ => PlatformError::Storage,
        }
    }
}
