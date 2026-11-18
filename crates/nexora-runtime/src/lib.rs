//! Concrete local database adapters, separate from the analysis core.
pub use nexora_database_connections::DatabaseConnections;
mod bounded_json;
mod cache_retention;
mod connection_secrets;
pub use cache_retention::*;
mod dragonfly_cache;
pub use dragonfly_cache::*;
#[cfg(test)]
mod postgres_snapshot_size_tests;
mod postgres_snapshots;
mod repository_lease;
mod snapshot_checksum;
mod snapshot_payload;
#[cfg(test)]
mod snapshot_payload_tests;
pub use postgres_snapshots::*;
mod neo4j_client;
mod neo4j_projection;
pub use neo4j_projection::*;
mod graph_traversal;
pub use graph_traversal::*;
mod persistent_index_result;
pub use persistent_index_result::*;
mod database_runtime;
pub use database_runtime::*;
#[cfg(test)]
mod dragonfly_cache_tests;

mod knowledge_query;
pub use knowledge_query::*;
mod query_snapshot;
pub use query_snapshot::*;
mod query_metrics;
pub use query_metrics::*;
mod neo4j_queries;
mod postgres_query_index;

mod query_cache_record;
mod query_result_cache;
pub use query_result_cache::*;

mod source_line_checksum;

mod knowledge_anchor;
pub use knowledge_anchor::*;
mod knowledge_draft;
pub use knowledge_draft::*;
mod knowledge_command;
pub use knowledge_command::*;

mod knowledge_evidence;
mod knowledge_service;
mod postgres_knowledge;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_environment;
