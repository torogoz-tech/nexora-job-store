/// A delivery claim loaded from the durable notification queue.
pub struct EmailNotification {
    pub event_key: String,
    pub kind: String,
    pub project_id: Option<String>,
    pub resource_id: String,
    pub actor_id: String,
    pub email: String,
    pub attempts: i32,
    pub language: String,
}
