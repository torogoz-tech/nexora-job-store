pub struct CodeSnapshotEmbedding {
    pub path: String,
    pub sequence: i32,
    pub title: String,
    pub content: String,
    pub content_hash: String,
    pub model: String,
    pub values: Vec<f32>,
}
