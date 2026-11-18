pub(crate) struct ApprovalOperationSnapshot {
    pub target_kind: &'static str,
    pub resource_id: String,
    pub version_id: Option<String>,
    pub transition_id: String,
    pub root_revision: i64,
    pub resource_revision: i64,
    pub workflow_version_id: String,
    pub from_state: String,
    pub to_state: String,
    pub checksum: String,
}
