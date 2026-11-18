/// Server-derived worker inputs. Not accepted from an HTTP or MCP request.
pub struct RepositoryRuntimeScope {
    pub tenant_id: String,
    pub repository_id: String,
    pub pinned_commit: String,
}
