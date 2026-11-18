use crate::{
    KnowledgeDraft, KnowledgeQuery, Neo4jProjection, QuerySnapshot, neo4j_queries::Neo4jQueries,
    postgres_query_index::PostgresQueryIndex,
};
use nexora_core::NexoraError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(crate) struct KnowledgeEvidence;
impl KnowledgeEvidence {
    pub fn verify(
        client: &mut impl postgres::GenericClient,
        graph: &Neo4jProjection,
        head: &QuerySnapshot,
        draft: &KnowledgeDraft,
    ) -> Result<Value, NexoraError> {
        draft.validate()?;
        for node in [&draft.source, &draft.target].into_iter().flatten() {
            Neo4jQueries::new(graph).node(head, node)?;
        }
        let mut anchors = Vec::new();
        for anchor in &draft.evidence {
            let value = PostgresQueryIndex::query(
                client,
                head,
                &KnowledgeQuery::Context {
                    path: anchor.path.clone(),
                    start_line: anchor.start_line,
                    line_count: anchor.line_count,
                },
            )?;
            let lines = value["items"]
                .as_array()
                .ok_or_else(|| NexoraError::Storage("knowledge evidence unavailable".into()))?;
            if lines.len() != anchor.line_count
                || lines.iter().any(|line| line["text_truncated"] == true)
            {
                return Err(NexoraError::Configuration(
                    "knowledge evidence range missing or truncated; narrow the anchor".into(),
                ));
            }
            let bytes = serde_json::to_vec(&value)
                .map_err(|_| NexoraError::Storage("knowledge evidence encoding failed".into()))?;
            anchors.push(
                json!({"anchor":anchor,"context_hash":format!("{:x}",Sha256::digest(bytes))}),
            );
        }
        Ok(json!(anchors))
    }
}
