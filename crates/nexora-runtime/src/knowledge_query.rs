use nexora_core::{NexoraError, NodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KnowledgeQuery {
    Overview {
        offset: usize,
        limit: usize,
    },
    FindSymbol {
        name: String,
        offset: usize,
        limit: usize,
    },
    References {
        node: NodeId,
        offset: usize,
        limit: usize,
    },
    Dependencies {
        node: NodeId,
        offset: usize,
        limit: usize,
    },
    Traverse {
        node: NodeId,
        depth: usize,
        limit: usize,
    },
    Search {
        query: String,
        offset: usize,
        limit: usize,
    },
    Context {
        path: String,
        start_line: usize,
        line_count: usize,
    },
}
impl KnowledgeQuery {
    pub fn validate(&self) -> Result<(), NexoraError> {
        let error = || NexoraError::Limit("query arguments outside supported bounds".into());
        match self {
            Self::Context {
                path,
                start_line,
                line_count,
            } => {
                if path.is_empty()
                    || path.len() > 4096
                    || path.contains('\0')
                    || *start_line == 0
                    || *start_line > 1_000_000
                    || *line_count == 0
                    || *line_count > 200
                {
                    return Err(error());
                }
            }
            Self::Traverse { node, depth, limit } => {
                if node.0.is_empty()
                    || node.0.len() > 4096
                    || *depth > 32
                    || *limit == 0
                    || *limit > 1000
                {
                    return Err(error());
                }
            }
            _ => {
                let (offset, limit) = self.page();
                if offset > 1_000_000 || limit == 0 || limit > 1000 {
                    return Err(error());
                }
                let text = match self {
                    Self::FindSymbol { name, .. } => Some(name.as_str()),
                    Self::References { node, .. } | Self::Dependencies { node, .. } => {
                        Some(node.0.as_str())
                    }
                    Self::Search { query, .. } => Some(query.as_str()),
                    _ => None,
                };
                if text.is_some_and(|s| s.is_empty() || s.len() > 4096) {
                    return Err(error());
                }
            }
        }
        Ok(())
    }
    pub(crate) fn page(&self) -> (usize, usize) {
        match self {
            Self::Overview { offset, limit }
            | Self::FindSymbol { offset, limit, .. }
            | Self::References { offset, limit, .. }
            | Self::Dependencies { offset, limit, .. }
            | Self::Search { offset, limit, .. } => (*offset, *limit),
            _ => (0, 0),
        }
    }
    pub(crate) fn backend(&self) -> &'static str {
        match self {
            Self::FindSymbol { .. }
            | Self::References { .. }
            | Self::Dependencies { .. }
            | Self::Traverse { .. } => "neo4j",
            _ => "postgresql",
        }
    }
}
