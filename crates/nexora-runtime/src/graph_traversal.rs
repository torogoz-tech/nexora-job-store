use serde::Serialize;
/// Identities refer to the full facts and evidence in the canonical snapshot.
#[derive(Debug, Serialize)]
pub struct GraphTraversal {
    pub node_ids: Vec<String>,
    pub edge_ids: Vec<String>,
    pub truncated: bool,
}
