pub struct KnowledgeSynthesisJob {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub payload: Vec<u8>,
    pub source_revision: i64,
    pub language: String,
}
