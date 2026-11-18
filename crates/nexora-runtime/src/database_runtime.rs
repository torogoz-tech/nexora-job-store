use crate::{
    DatabaseConnections, DragonflyCache, GraphTraversal, Neo4jProjection, PersistentIndexResult,
    PostgresSnapshots, connection_secrets::ConnectionSecrets, snapshot_checksum::SnapshotChecksum,
};
use nexora_core::{NexoraError, NodeId, RepositoryId, SnapshotId, TenantId};
use nexora_indexer::{ExternalFileCache, Indexer, MemoryStore};
use nexora_storage::Snapshot;
use std::{cell::RefCell, path::Path};
pub struct DatabaseRuntime {
    postgres: PostgresSnapshots,
    cache: RefCell<DragonflyCache>,
    graph: Neo4jProjection,
    query_metrics: Option<crate::QueryMetrics>,
    knowledge_initialized: bool,
}
impl DatabaseRuntime {
    pub fn connect(c: &DatabaseConnections) -> Result<Self, NexoraError> {
        c.validate()?;
        let secrets = ConnectionSecrets::load(c)?;
        Ok(Self {
            postgres: PostgresSnapshots::connect(c, &secrets)?,
            cache: RefCell::new(DragonflyCache::connect(c, &secrets)?),
            graph: Neo4jProjection::new(c, &secrets),
            query_metrics: None,
            knowledge_initialized: false,
        })
    }
    pub fn check(&mut self) -> Result<(), NexoraError> {
        self.postgres.ping()?;
        self.cache.borrow_mut().ping()?;
        self.graph.ping()
    }
    pub fn index(
        &mut self,
        indexer: &Indexer,
        path: &Path,
        tenant: TenantId,
    ) -> Result<PersistentIndexResult, NexoraError> {
        self.index_impl(indexer, path, tenant, None, true)
    }
    /// Platform-only entry point; identity comes from an authorized repository registration.
    pub fn index_repository(
        &mut self,
        indexer: &Indexer,
        path: &Path,
        tenant: TenantId,
        repository: RepositoryId,
    ) -> Result<PersistentIndexResult, NexoraError> {
        self.index_impl(indexer, path, tenant, Some(repository), true)
    }
    /// Prepares immutable projections without moving the active repository head.
    /// The platform must authorize and commit its explicit snapshot binding separately.
    pub fn stage_repository(
        &mut self,
        indexer: &Indexer,
        path: &Path,
        tenant: TenantId,
        repository: RepositoryId,
    ) -> Result<PersistentIndexResult, NexoraError> {
        self.index_impl(indexer, path, tenant, Some(repository), false)
    }
    /// Stages one aggregate multi-profile snapshot without moving the active head.
    pub fn stage_repository_profiles(
        &mut self,
        indexer: &nexora_indexer::MultiProfileIndexer,
        path: &Path,
        tenant: TenantId,
        repository: RepositoryId,
    ) -> Result<(PersistentIndexResult, serde_json::Value), NexoraError> {
        let started = std::time::Instant::now();
        let result = {
            let mut read = |key: &nexora_indexer::FileCacheKey| self.cache.borrow_mut().read(key);
            let mut write = |key: &nexora_indexer::FileCacheKey,
                             fragment: &nexora_indexer::CacheFragment| {
                self.cache.borrow_mut().write(key, fragment)
            };
            indexer.index_repository_external(
                path,
                tenant,
                repository,
                &mut MemoryStore::default(),
                &mut ExternalFileCache {
                    read: &mut read,
                    write: &mut write,
                },
            )?
        };
        let storage = std::time::Instant::now();
        self.publish(&result.index.snapshot, false)?;
        let storage_ms = storage.elapsed().as_secs_f64() * 1000.0;
        Ok((
            PersistentIndexResult {
                snapshot: result.index.snapshot,
                reuse: result.index.reuse,
                timings: result.index.timings,
                storage_ms,
                operation_ms: started.elapsed().as_secs_f64() * 1000.0,
            },
            result.selection,
        ))
    }
    fn index_impl(
        &mut self,
        indexer: &Indexer,
        path: &Path,
        tenant: TenantId,
        repository: Option<RepositoryId>,
        activate: bool,
    ) -> Result<PersistentIndexResult, NexoraError> {
        let started = std::time::Instant::now();
        let result = {
            let mut read = |key: &nexora_indexer::FileCacheKey| self.cache.borrow_mut().read(key);
            let mut write = |key: &nexora_indexer::FileCacheKey,
                             fragment: &nexora_indexer::CacheFragment| {
                self.cache.borrow_mut().write(key, fragment)
            };
            let mut callbacks = ExternalFileCache {
                read: &mut read,
                write: &mut write,
            };
            match repository {
                Some(repository) => indexer.index_repository_external(
                    path,
                    tenant,
                    repository,
                    &mut MemoryStore::default(),
                    &mut callbacks,
                )?,
                None => indexer.index_external(
                    path,
                    tenant,
                    &mut MemoryStore::default(),
                    &mut callbacks,
                )?,
            }
        };
        let storage = std::time::Instant::now();
        self.publish(&result.snapshot, activate)?;
        let storage_ms = storage.elapsed().as_secs_f64() * 1000.0;
        Ok(PersistentIndexResult {
            snapshot: result.snapshot,
            reuse: result.reuse,
            timings: result.timings,
            storage_ms,
            operation_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
    pub fn publish(&mut self, s: &Snapshot, activate: bool) -> Result<(), NexoraError> {
        self.postgres.initialize()?;
        let _lease = self.postgres.lease(&s.tenant_id, &s.repository_id)?;
        let hash = self.postgres.prepare(s)?;
        self.graph.project(s, &hash)?;
        crate::postgres_query_index::PostgresQueryIndex::project(self.postgres.client(), s, &hash)?;
        self.postgres.ready(s, &hash, activate)
    }
    pub fn snapshot(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
    ) -> Result<Option<Snapshot>, NexoraError> {
        self.postgres.get(t, r, id)
    }
    /// Expensive operator verification; ordinary authorized queries use headers only.
    pub fn verify_persisted_snapshot(
        &mut self,
        tenant: &TenantId,
        repository: &RepositoryId,
        snapshot: &SnapshotId,
        expected_hash: &str,
    ) -> Result<crate::QuerySnapshot, NexoraError> {
        let stored = self
            .postgres
            .get(tenant, repository, Some(snapshot))?
            .ok_or_else(|| NexoraError::Storage("local snapshot unavailable".into()))?;
        let hash = SnapshotChecksum::calculate(&stored)?;
        let header = self.query_header(tenant, repository, Some(snapshot))?;
        if hash != expected_hash || header.payload_hash != hash {
            return Err(NexoraError::Storage(
                "local snapshot checksum mismatch".into(),
            ));
        }
        self.graph.verify_snapshot(&stored, &hash)?;
        Ok(header)
    }
    pub fn traverse(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: &SnapshotId,
        node: &NodeId,
        depth: usize,
        limit: usize,
    ) -> Result<GraphTraversal, NexoraError> {
        let head = self.query_header(t, r, Some(id))?;
        let scope = Neo4jProjection::scope(t, r, id)?;
        self.graph
            .traverse_scope(&scope, &head.payload_hash, node, depth, limit)
    }
    /// Explicit administrative preparation; ordinary queries never call this method.
    pub fn prepare_queries(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
    ) -> Result<(), NexoraError> {
        self.postgres.initialize()?;
        let _lease = self.postgres.lease(t, r)?;
        let s = self
            .postgres
            .get(t, r, id)?
            .ok_or_else(|| NexoraError::Storage("published snapshot not found".into()))?;
        let hash = SnapshotChecksum::calculate(&s)?;
        self.graph.project(&s, &hash)?;
        crate::postgres_query_index::PostgresQueryIndex::project(self.postgres.client(), &s, &hash)
    }
    pub fn query_header(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
    ) -> Result<crate::QuerySnapshot, NexoraError> {
        crate::postgres_query_index::PostgresQueryIndex::head(self.postgres.client(), t, r, id)
    }
    pub fn knowledge(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
        actor: &str,
        command: &crate::KnowledgeCommand,
    ) -> Result<serde_json::Value, NexoraError> {
        command.validate()?;
        if !self.knowledge_initialized {
            self.postgres.initialize()?;
            self.knowledge_initialized = true;
        }
        crate::knowledge_service::KnowledgeService::execute(
            &mut self.postgres,
            &self.graph,
            t,
            r,
            id,
            actor,
            command,
        )
    }
    /// Appends knowledge inside the caller's transaction; the caller owns commit/rollback.
    pub fn knowledge_in_transaction(
        &mut self,
        tx: &mut postgres::Transaction<'_>,
        head: &crate::QuerySnapshot,
        actor: &str,
        command: &crate::KnowledgeCommand,
    ) -> Result<serde_json::Value, NexoraError> {
        command.validate()?;
        if !command.is_write() {
            return Err(NexoraError::Configuration(
                "knowledge mutation required".into(),
            ));
        }
        if !self.knowledge_initialized {
            self.postgres.initialize()?;
            self.knowledge_initialized = true;
        }
        crate::repository_lease::RepositoryLease::lock_transaction(
            tx,
            &head.tenant_id,
            &head.repository_id,
        )?;
        crate::knowledge_service::KnowledgeService::write(
            tx,
            &self.graph,
            head,
            Some(&head.id),
            actor,
            command,
        )
    }
    pub fn knowledge_pending(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
        offset: usize,
        limit: usize,
    ) -> Result<serde_json::Value, NexoraError> {
        if offset > 1_000_000 || limit == 0 || limit > 100 {
            return Err(NexoraError::Limit("pending page outside bounds".into()));
        }
        let head = self.query_header(t, r, id)?;
        crate::neo4j_queries::Neo4jQueries::new(&self.graph).pending(&head, offset, limit)
    }
    pub fn query_metrics(&self) -> Option<&crate::QueryMetrics> {
        self.query_metrics.as_ref()
    }
    pub fn query(
        &mut self,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
        query: &crate::KnowledgeQuery,
    ) -> Result<serde_json::Value, NexoraError> {
        query.validate()?;
        let started = std::time::Instant::now();
        let head = self.query_header(t, r, id)?;
        let mut metrics = crate::QueryMetrics {
            backend: query.backend().into(),
            metadata_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
            ..Default::default()
        };
        let key = crate::QueryResultCache::key(&head, query)?;
        let cache_started = std::time::Instant::now();
        let cached = crate::QueryResultCache::read(&mut self.cache.borrow_mut(), &key);
        metrics.cache_us = cache_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        metrics.cache_available = cached.is_ok();
        if cached.is_err() {
            self.cache.borrow_mut().reset_connection();
        }
        if let Ok(Some(value)) = cached {
            metrics.cache_hit = true;
            self.query_metrics = Some(metrics);
            return Ok(value);
        }
        let backend_started = std::time::Instant::now();
        let value = if query.backend() == "neo4j" {
            crate::neo4j_queries::Neo4jQueries::new(&self.graph).query(&head, query)?
        } else {
            crate::postgres_query_index::PostgresQueryIndex::query(
                self.postgres.client(),
                &head,
                query,
            )?
        };
        metrics.backend_us = backend_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        if metrics.cache_available
            && crate::QueryResultCache::write(
                &mut self.cache.borrow_mut(),
                &key,
                &value,
                metrics.backend_us,
            )
            .is_err()
        {
            metrics.cache_available = false;
            self.cache.borrow_mut().reset_connection();
        }
        self.query_metrics = Some(metrics);
        Ok(value)
    }
}
#[cfg(test)]
#[path = "selective_runtime_tests.rs"]
mod selective_runtime_tests;
