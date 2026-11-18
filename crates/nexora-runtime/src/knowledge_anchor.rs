use nexora_core::NexoraError;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeAnchor {
    pub path: String,
    pub start_line: usize,
    pub line_count: usize,
}
impl KnowledgeAnchor {
    pub fn validate(&self) -> Result<(), NexoraError> {
        if self.path.is_empty()
            || self.path.len() > 4096
            || self.path.contains(['\0', '\\'])
            || self.path.starts_with('/')
            || self.path.split('/').any(|p| matches!(p, "" | "." | ".."))
            || self.start_line == 0
            || self.start_line > 1_000_000
            || self.line_count == 0
            || self.line_count > 200
        {
            return Err(NexoraError::Configuration(
                "invalid knowledge evidence anchor".into(),
            ));
        }
        Ok(())
    }
}
