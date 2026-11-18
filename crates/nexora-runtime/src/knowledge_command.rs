use crate::KnowledgeDraft;
use nexora_core::NexoraError;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeCommand {
    Contribute {
        request_id: String,
        draft: KnowledgeDraft,
    },
    Revise {
        id: String,
        request_id: String,
        expected_revision: u32,
        draft: KnowledgeDraft,
    },
    Review {
        id: String,
        request_id: String,
        expected_revision: u32,
        action: String,
        reason: String,
    },
    List {
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        node: Option<String>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "KnowledgeCommand::default_limit")]
        limit: usize,
    },
    Get {
        id: String,
        #[serde(default)]
        offset: usize,
        #[serde(default = "KnowledgeCommand::default_limit")]
        limit: usize,
    },
}
impl KnowledgeCommand {
    fn default_limit() -> usize {
        50
    }
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Self::Contribute { .. } | Self::Revise { .. } | Self::Review { .. }
        )
    }
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Contribute { request_id, .. }
            | Self::Revise { request_id, .. }
            | Self::Review { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<(), NexoraError> {
        let error = || NexoraError::Configuration("invalid knowledge command".into());
        let key =
            |s: &str| !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control);
        if self.request_id().is_some_and(|s| !key(s)) {
            return Err(error());
        }
        match self {
            Self::Contribute { draft, .. } => draft.validate()?,
            Self::Revise {
                id,
                expected_revision,
                draft,
                ..
            } => {
                if !key(id) || *expected_revision == 0 || *expected_revision >= i32::MAX as u32 {
                    return Err(error());
                }
                draft.validate()?;
            }
            Self::Review {
                id,
                expected_revision,
                action,
                reason,
                ..
            } => {
                if !key(id)
                    || *expected_revision == 0
                    || *expected_revision >= i32::MAX as u32
                    || !matches!(action.as_str(), "accept" | "reject" | "withdraw")
                    || reason.trim().is_empty()
                    || reason.len() > 4096
                {
                    return Err(error());
                }
            }
            Self::List {
                status,
                node,
                offset,
                limit,
            } => {
                if status.as_deref().is_some_and(|s| {
                    !matches!(
                        s,
                        "proposed" | "accepted" | "rejected" | "withdrawn" | "needs_review"
                    )
                }) || node
                    .as_ref()
                    .is_some_and(|n| n.is_empty() || n.len() > 4096 || n.contains('\0'))
                    || *offset > 1_000_000
                    || *limit == 0
                    || *limit > 100
                {
                    return Err(error());
                }
            }
            Self::Get { id, offset, limit } => {
                if !key(id) || *offset > 1_000_000 || *limit == 0 || *limit > 100 {
                    return Err(error());
                }
            }
        }
        Ok(())
    }
}
