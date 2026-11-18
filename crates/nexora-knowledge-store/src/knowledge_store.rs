use crate::{
    KnowledgeEmbeddingRecord, KnowledgeProcessingJob, KnowledgeProcessingSource,
    knowledge_store_secret::KnowledgeStoreSecret,
};
use nexora_database_connections::{DatabaseConnections, PostgresConnection};
use nexora_platform::PlatformError;
use postgres::{Client, error::SqlState};
use serde_json::Value;

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

    pub fn knowledge_processing_source(
        &mut self,
        job: &KnowledgeProcessingJob,
    ) -> Result<KnowledgeProcessingSource, PlatformError> {
        let (title, body, kind) = match job.source_kind.as_str() {
            "document" | "technical_debt" => self
                .client()
                .query_opt(
                    "SELECT title,body FROM knowledge_contributions WHERE id=$1 AND revision=$2",
                    &[&job.resource_id, &job.source_revision],
                )
                .map_err(Self::database_error)?
                .map(|row| {
                    (
                        row.get(0),
                        row.get(1),
                        job.source_kind.clone(),
                    )
                })
                .ok_or(PlatformError::NotFound)?,
            "contribution" => self
                .client()
                .query_opt(
                    "SELECT title,body,kind FROM knowledge_contributions WHERE id=$1 AND revision=$2",
                    &[&job.resource_id, &job.source_revision],
                )
                .map_err(Self::database_error)?
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .ok_or(PlatformError::NotFound)?,
            "organization_document" => self
                .client()
                .query_opt(
                    "SELECT title,body FROM organization_document_versions WHERE document_id=$1 AND revision=$2",
                    &[&job.resource_id, &job.source_revision],
                )
                .map_err(Self::database_error)?
                .map(|row| (row.get(0), row.get(1), job.source_kind.clone()))
                .ok_or(PlatformError::NotFound)?,
            "project_document" => self
                .client()
                .query_opt(
                    "SELECT title,content::text FROM project_document_versions WHERE document_id=$1 AND number=$2",
                    &[&job.resource_id, &job.source_revision],
                )
                .map_err(Self::database_error)?
                .map(|row| (row.get(0), row.get(1), job.source_kind.clone()))
                .ok_or(PlatformError::NotFound)?,
            _ => return Err(PlatformError::Validation("unsupported knowledge source")),
        };

        Ok(KnowledgeProcessingSource { title, body, kind })
    }

    pub fn finish_knowledge_processing(
        &mut self,
        job: &KnowledgeProcessingJob,
        structure: &Value,
        chunks: &[crate::KnowledgeChunkRecord],
    ) -> Result<(), PlatformError> {
        if !structure.is_object() || structure["summary"].as_str().is_none() {
            return Err(PlatformError::Validation("invalid knowledge structure"));
        }
        let structure_bytes = serde_json::to_vec(structure).map_err(|_| PlatformError::Storage)?;
        let structure_hash = KnowledgeStoreSecret::checksum(&structure_bytes);
        let lease_digest = KnowledgeStoreSecret::digest(&job.lease_token);
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        let status = transaction
            .query_opt(
                "SELECT status FROM knowledge_processing_jobs WHERE id=$1 AND source_kind=$2 AND resource_id=$3 AND source_revision=$4 AND lease_token_digest=$5 AND lease_until>clock_timestamp() FOR UPDATE",
                &[&job.id, &job.source_kind, &job.resource_id, &job.source_revision, &lease_digest],
            )
            .map_err(Self::database_error)?
            .map(|row| row.get::<_, String>(0));
        if status.as_deref() != Some("processing") {
            return Err(PlatformError::Conflict(
                "knowledge processing job is no longer active",
            ));
        }

        transaction
            .execute(
                "INSERT INTO knowledge_structures(job_id,source_kind,resource_id,source_revision,schema_version,structure,structure_hash,validation_status) VALUES($1,$2,$3,$4,'1.0',$5,$6,'valid') ON CONFLICT(job_id) DO UPDATE SET structure=$5,structure_hash=$6,validation_status='valid'",
                &[&job.id, &job.source_kind, &job.resource_id, &job.source_revision, structure, &structure_hash],
            )
            .map_err(Self::database_error)?;
        for chunk in chunks {
            transaction
                .execute(
                    "INSERT INTO knowledge_chunks(id,job_id,project_id,source_kind,resource_id,source_revision,sequence,title,content,content_hash,published) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,false) ON CONFLICT(id) DO NOTHING",
                    &[&chunk.id, &job.id, &job.project_id, &job.source_kind, &job.resource_id, &job.source_revision, &chunk.sequence, &chunk.title, &chunk.content, &chunk.content_hash],
                )
                .map_err(Self::database_error)?;
        }
        transaction
            .execute(
                "UPDATE knowledge_processing_jobs SET status='structured',updated_at=clock_timestamp(),last_error=NULL WHERE id=$1 AND status='processing' AND lease_token_digest=$2 AND lease_until>clock_timestamp()",
                &[&job.id, &lease_digest],
            )
            .map_err(Self::database_error)?;
        transaction.commit().map_err(Self::database_error)?;
        Ok(())
    }

    pub fn store_knowledge_embeddings(
        &mut self,
        job: &KnowledgeProcessingJob,
        embedding_model: &str,
        embeddings: &[KnowledgeEmbeddingRecord],
    ) -> Result<bool, PlatformError> {
        if embedding_model.is_empty() || embedding_model.len() > 128 || embeddings.is_empty() {
            return Err(PlatformError::Validation("invalid knowledge embeddings"));
        }
        let lease_digest = KnowledgeStoreSecret::digest(&job.lease_token);
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        let status = transaction
            .query_opt(
                "SELECT status FROM knowledge_processing_jobs WHERE id=$1 AND lease_token_digest=$2 AND lease_until>clock_timestamp() FOR UPDATE",
                &[&job.id, &lease_digest],
            )
            .map_err(Self::database_error)?
            .map(|row| row.get::<_, String>(0));
        if status.as_deref() != Some("structured") {
            transaction.rollback().map_err(Self::database_error)?;
            return Ok(false);
        }

        let mut chunk_ids = std::collections::HashSet::new();
        for embedding in embeddings {
            if embedding.values.is_empty()
                || embedding.values.len() > i32::MAX as usize
                || embedding.content_hash.is_empty()
                || embedding.content_hash.len() > 128
                || embedding.values.iter().any(|value| !value.is_finite())
                || !chunk_ids.insert(embedding.chunk_id.as_str())
            {
                return Err(PlatformError::Validation("invalid knowledge embedding"));
            }
            let literal = format!(
                "[{}]",
                embedding
                    .values
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let dimension = embedding.values.len() as i32;
            let changed = transaction
                .execute(
                    "INSERT INTO knowledge_embeddings(chunk_id,embedding_model,dimension,embedding,content_hash,published) SELECT c.id,$2,$3,$4::text::vector,$5,false FROM knowledge_chunks c WHERE c.id=$1 AND c.job_id=$6 AND c.content_hash=$5 ON CONFLICT(chunk_id,embedding_model,dimension) DO UPDATE SET embedding=EXCLUDED.embedding,content_hash=EXCLUDED.content_hash,published=false,created_at=clock_timestamp()",
                    &[&embedding.chunk_id, &embedding_model, &dimension, &literal, &embedding.content_hash, &job.id],
                )
                .map_err(Self::database_error)?;
            if changed != 1 {
                return Err(PlatformError::Conflict(
                    "knowledge embedding does not match a current chunk",
                ));
            }
        }
        transaction.commit().map_err(Self::database_error)?;
        Ok(true)
    }

    pub fn publish_knowledge_embeddings(
        &mut self,
        job: &KnowledgeProcessingJob,
        embedding_model: &str,
    ) -> Result<bool, PlatformError> {
        if embedding_model.is_empty() || embedding_model.len() > 128 {
            return Err(PlatformError::Validation("invalid embedding model"));
        }
        let lease_digest = KnowledgeStoreSecret::digest(&job.lease_token);
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        let status = transaction
            .query_opt(
                "SELECT status FROM knowledge_processing_jobs WHERE id=$1 AND lease_token_digest=$2 AND lease_until>clock_timestamp() FOR UPDATE",
                &[&job.id, &lease_digest],
            )
            .map_err(Self::database_error)?
            .map(|row| row.get::<_, String>(0));
        if status.as_deref() != Some("structured") {
            transaction.rollback().map_err(Self::database_error)?;
            return Ok(false);
        }
        let chunks: i64 = transaction
            .query_one(
                "SELECT count(*) FROM knowledge_chunks WHERE job_id=$1",
                &[&job.id],
            )
            .map_err(Self::database_error)?
            .get(0);
        if chunks == 0 {
            return Err(PlatformError::Validation(
                "knowledge job produced no chunks",
            ));
        }
        let missing_embeddings: bool = transaction
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM knowledge_chunks c WHERE c.job_id=$1 AND NOT EXISTS(SELECT 1 FROM knowledge_embeddings e WHERE e.chunk_id=c.id AND e.embedding_model=$2 AND e.content_hash=c.content_hash))",
                &[&job.id, &embedding_model],
            )
            .map_err(Self::database_error)?
            .get(0);
        if missing_embeddings {
            return Err(PlatformError::Conflict(
                "knowledge embeddings are incomplete or stale",
            ));
        }
        transaction
            .execute(
                "UPDATE knowledge_embeddings e SET published=true,created_at=clock_timestamp() FROM knowledge_chunks c WHERE c.id=e.chunk_id AND c.job_id=$1 AND c.content_hash=e.content_hash AND e.embedding_model=$2",
                &[&job.id, &embedding_model],
            )
            .map_err(Self::database_error)?;
        transaction
            .execute(
                "UPDATE knowledge_chunks SET published=true WHERE job_id=$1",
                &[&job.id],
            )
            .map_err(Self::database_error)?;
        let changed = transaction
            .execute(
                "UPDATE knowledge_processing_jobs SET status='indexed',lease_owner=NULL,lease_token_digest=NULL,lease_until=NULL,updated_at=clock_timestamp(),last_error=NULL WHERE id=$1 AND status='structured' AND lease_token_digest=$2 AND lease_until>clock_timestamp()",
                &[&job.id, &lease_digest],
            )
            .map_err(Self::database_error)?;
        if changed != 1 {
            return Err(PlatformError::Conflict(
                "knowledge processing lease expired before publication",
            ));
        }
        transaction.commit().map_err(Self::database_error)?;
        Ok(true)
    }

    pub(crate) fn client(&mut self) -> &mut Client {
        self.connection.client()
    }

    pub(crate) fn database_error(error: postgres::Error) -> PlatformError {
        eprintln!("knowledge store database error: {error:?}");
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
