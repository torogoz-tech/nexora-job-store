use crate::{KnowledgeStore, KnowledgeSynthesisJob, knowledge_store_secret::KnowledgeStoreSecret};
use nexora_platform::PlatformError;

impl KnowledgeStore {
    pub fn claim_knowledge_synthesis(
        &mut self,
    ) -> Result<Option<KnowledgeSynthesisJob>, PlatformError> {
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        transaction
            .execute(
                "UPDATE knowledge_contributions SET status='failed',updated_at=now() WHERE status='processing' AND claimed_at<now()-interval '3 minutes'",
                &[],
            )
            .map_err(Self::database_error)?;
        let row = transaction
            .query_opt(
                "SELECT c.id,c.filename,c.mime_type,src.payload,src.sha256,c.source_revision,(SELECT language FROM installation_preferences LIMIT 1) FROM knowledge_contributions c JOIN knowledge_contribution_sources src ON src.contribution_id=c.id AND src.source_revision=c.source_revision JOIN actors a ON a.id=src.created_by WHERE c.status='queued' AND a.active AND EXISTS(SELECT 1 FROM actor_roles r WHERE r.actor_id=a.id) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='knowledge:contribute' AND (g.project_id IS NULL OR g.project_id=c.project_id)) AND ((c.project_id IS NULL AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='organization-knowledge:read' AND g.project_id IS NULL)) OR (c.project_id IS NOT NULL AND EXISTS(SELECT 1 FROM projects p WHERE p.id=c.project_id AND NOT p.archived) AND EXISTS(SELECT 1 FROM actor_grants g WHERE g.actor_id=a.id AND g.capability='documents:read' AND (g.project_id IS NULL OR g.project_id=c.project_id)))) ORDER BY c.created_at,c.id FOR UPDATE OF c SKIP LOCKED LIMIT 1",
                &[],
            )
            .map_err(Self::database_error)?;
        let Some(row) = row else {
            transaction.commit().map_err(Self::database_error)?;
            return Ok(None);
        };
        let payload: Vec<u8> = row.get(3);
        let checksum: String = row.get(4);
        if KnowledgeStoreSecret::checksum(&payload) != checksum {
            return Err(PlatformError::Storage);
        }
        let id: String = row.get(0);
        transaction
            .execute(
                "UPDATE knowledge_contributions SET status='processing',attempts=attempts+1,claimed_at=now(),updated_at=now() WHERE id=$1",
                &[&id],
            )
            .map_err(Self::database_error)?;
        let job = KnowledgeSynthesisJob {
            id,
            filename: row.get(1),
            mime_type: row.get(2),
            payload,
            source_revision: row.get(5),
            language: row
                .get::<_, Option<String>>(6)
                .unwrap_or_else(|| "es".to_owned()),
        };
        transaction.commit().map_err(Self::database_error)?;
        Ok(Some(job))
    }

    pub fn finish_knowledge_synthesis(
        &mut self,
        job: &KnowledgeSynthesisJob,
        body: Option<&str>,
    ) -> Result<bool, PlatformError> {
        let body = body.filter(|value| !value.trim().is_empty() && value.len() <= 16_000);
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        let changed = if let Some(body) = body {
            let changed = transaction
                .execute(
                    "UPDATE knowledge_contributions SET body=$2,status='ready',revision=revision+1,updated_at=now() WHERE id=$1 AND status='processing' AND source_revision=$3",
                    &[&job.id, &body, &job.source_revision],
                )
                .map_err(Self::database_error)?;
            if changed == 1 {
                transaction
                    .execute(
                        "INSERT INTO knowledge_contribution_versions(contribution_id,revision,source_revision,title,body) SELECT id,revision,source_revision,title,body FROM knowledge_contributions WHERE id=$1",
                        &[&job.id],
                    )
                    .map_err(Self::database_error)?;
                transaction
                    .execute(
                        "INSERT INTO knowledge_processing_jobs(id,project_id,source_kind,resource_id,source_revision,content_hash,status) SELECT 'krag-'||id||'-'||revision,project_id,kind,id,revision,COALESCE(sha256,md5(title||body)),'queued' FROM knowledge_contributions WHERE id=$1 ON CONFLICT(source_kind,resource_id,source_revision,content_hash) DO UPDATE SET status=CASE WHEN knowledge_processing_jobs.status IN ('failed','cancelled') THEN 'queued' ELSE knowledge_processing_jobs.status END,updated_at=clock_timestamp()",
                        &[&job.id],
                    )
                    .map_err(Self::database_error)?;
            }
            changed == 1
        } else {
            transaction
                .execute(
                    "UPDATE knowledge_contributions SET status='failed',updated_at=now() WHERE id=$1 AND status='processing' AND source_revision=$2",
                    &[&job.id, &job.source_revision],
                )
                .map_err(Self::database_error)?
                == 1
        };
        transaction.commit().map_err(Self::database_error)?;
        Ok(changed)
    }
}
