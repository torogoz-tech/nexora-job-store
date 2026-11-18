pub struct KnowledgeProcessingJob {
    pub id: String,
    pub project_id: Option<String>,
    pub source_kind: String,
    pub resource_id: String,
    pub source_revision: i64,
    pub content_hash: String,
    pub lease_token: String,
}
