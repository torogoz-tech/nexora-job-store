use crate::KnowledgeAnchor;
use nexora_core::NexoraError;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeDraft {
    pub kind: String,
    pub statement: String,
    pub evidence: Vec<KnowledgeAnchor>,
    pub source: Option<String>,
    pub target: Option<String>,
    pub relation: Option<String>,
}
impl KnowledgeDraft {
    pub fn validate(&self) -> Result<(), NexoraError> {
        let error = || NexoraError::Configuration("invalid knowledge draft".into());
        if !matches!(self.kind.as_str(), "relation" | "explanation" | "decision")
            || self.statement.trim().is_empty()
            || self.statement.len() > 4096
            || self.evidence.is_empty()
            || self.evidence.len() > 8
        {
            return Err(error());
        }
        for anchor in &self.evidence {
            anchor.validate()?;
        }
        for node in [&self.source, &self.target].into_iter().flatten() {
            if node.is_empty() || node.len() > 4096 || node.contains('\0') {
                return Err(error());
            }
        }
        if self.kind == "relation" {
            if self.source.is_none()
                || self.target.is_none()
                || self.relation.as_ref().is_none_or(|r| {
                    r.is_empty()
                        || r.len() > 128
                        || !r.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                })
            {
                return Err(error());
            }
        } else if self.target.is_some() || self.relation.is_some() {
            return Err(error());
        }
        Ok(())
    }
}
