use crate::{AnalysisStore, CodeSnapshotEmbedding};
use nexora_platform::PlatformError;

impl AnalysisStore {
    pub fn save_code_snapshot_embeddings(
        &mut self,
        project_id: &str,
        repository_id: &str,
        snapshot_id: &str,
        records: &[CodeSnapshotEmbedding],
    ) -> Result<(), PlatformError> {
        if records.len() > 64 {
            return Err(PlatformError::Validation("too many snapshot embeddings"));
        }
        if records.is_empty() {
            return Ok(());
        }

        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        for record in records {
            Self::validate_record(record)?;
            let vector = format!(
                "[{}]",
                record
                    .values
                    .iter()
                    .map(f32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let dimension = i32::try_from(record.values.len())
                .map_err(|_| PlatformError::Validation("invalid embedding dimension"))?;
            transaction
                .execute(
                    "INSERT INTO code_snapshot_embeddings(project_id,repository_id,snapshot_id,path,sequence,title,content,content_hash,embedding_model,dimension,embedding,published) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::vector,false) ON CONFLICT(project_id,repository_id,snapshot_id,path,sequence,embedding_model,dimension) DO UPDATE SET content=EXCLUDED.content,content_hash=EXCLUDED.content_hash,embedding=EXCLUDED.embedding,published=false,created_at=clock_timestamp()",
                    &[
                        &project_id,
                        &repository_id,
                        &snapshot_id,
                        &record.path,
                        &record.sequence,
                        &record.title,
                        &record.content,
                        &record.content_hash,
                        &record.model,
                        &dimension,
                        &vector,
                    ],
                )
                .map_err(Self::database_error)?;
        }
        transaction.commit().map_err(Self::database_error)
    }

    fn validate_record(record: &CodeSnapshotEmbedding) -> Result<(), PlatformError> {
        if record.path.is_empty()
            || record.path.len() > 4096
            || record.sequence < 0
            || record.title.is_empty()
            || record.title.len() > 4096
            || record.content.is_empty()
            || record.content.len() > 1_048_576
            || record.model.is_empty()
            || record.model.len() > 128
            || record.content_hash.len() != 64
            || !record
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || record.values.is_empty()
            || record.values.len() > 8192
            || record.values.iter().any(|value| !value.is_finite())
        {
            return Err(PlatformError::Validation("invalid code snapshot embedding"));
        }
        Ok(())
    }
}
