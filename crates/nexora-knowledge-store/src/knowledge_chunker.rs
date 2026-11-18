use crate::KnowledgeChunkRecord;
use nexora_platform::PlatformError;
use sha2::{Digest, Sha256};

pub struct KnowledgeChunker {
    max_chars: usize,
    overlap_chars: usize,
}

impl KnowledgeChunker {
    pub fn new(max_chars: usize, overlap_chars: usize) -> Result<Self, PlatformError> {
        if max_chars == 0 || overlap_chars >= max_chars {
            return Err(PlatformError::Validation("invalid knowledge chunk limits"));
        }

        Ok(Self {
            max_chars,
            overlap_chars,
        })
    }

    pub fn chunk(
        &self,
        resource_id: &str,
        revision: i64,
        title: &str,
        content: &str,
    ) -> Result<Vec<KnowledgeChunkRecord>, PlatformError> {
        let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let chars = normalized.chars().collect::<Vec<_>>();
        let mut chunks = Vec::new();
        let mut start = 0;

        while start < chars.len() {
            let end = (start + self.max_chars).min(chars.len());
            let text = chars[start..end].iter().collect::<String>();
            let sequence = i32::try_from(chunks.len())
                .map_err(|_| PlatformError::Validation("too many knowledge chunks"))?;
            let id = Self::hash(&format!("{resource_id}:{revision}:{sequence}:{text}"));
            let content_hash = Self::hash(&text);
            chunks.push(KnowledgeChunkRecord {
                id,
                sequence,
                title: title.to_owned(),
                content: text,
                content_hash,
            });

            if end == chars.len() {
                break;
            }

            start = end.saturating_sub(self.overlap_chars);
        }

        Ok(chunks)
    }

    fn hash(content: &str) -> String {
        Sha256::digest(content.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
