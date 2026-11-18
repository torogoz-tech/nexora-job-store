use crate::{
    DatabaseConnections, GraphTraversal, bounded_json::BoundedJson,
    connection_secrets::ConnectionSecrets, neo4j_client::Neo4jClient,
};
use nexora_core::{NexoraError, NodeId, RepositoryId, SnapshotId, TenantId};
use nexora_storage::Snapshot;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
pub struct Neo4jProjection {
    client: Neo4jClient,
}
impl Neo4jProjection {
    pub(crate) fn new(c: &DatabaseConnections, s: &ConnectionSecrets) -> Self {
        Self {
            client: Neo4jClient::new(c, s),
        }
    }
    fn error() -> NexoraError {
        NexoraError::Storage("invalid Neo4j projection".into())
    }
    pub fn scope(t: &TenantId, r: &RepositoryId, s: &SnapshotId) -> Result<String, NexoraError> {
        Ok(format!(
            "{:x}",
            Sha256::digest(BoundedJson::encode(&(t, r, s), 16_384)?)
        ))
    }
    fn remaining(start: Instant) -> Result<Duration, NexoraError> {
        Duration::from_secs(120)
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| NexoraError::Limit("graph operation >120s".into()))
    }
    pub(crate) fn client(&self) -> &Neo4jClient {
        &self.client
    }
    pub fn ping(&self) -> Result<(), NexoraError> {
        self.client
            .execute("RETURN 1", json!({}), Duration::from_secs(30))?;
        Ok(())
    }
    fn counts(&self, scope: &str, start: Instant) -> Result<(usize, usize), NexoraError> {
        let nodes = self.client.execute(
            "MATCH (n:NexoraNode {scope:$scope}) RETURN count(n)",
            json!({"scope":scope}),
            Self::remaining(start)?,
        )?;
        let edges=self.client.execute("MATCH (:NexoraNode {scope:$scope})-[r:NEXORA_EDGE {scope:$scope}]->(:NexoraNode {scope:$scope}) RETURN count(r)",json!({"scope":scope}),Self::remaining(start)?)?;
        let count = |rows: &Vec<Value>| {
            rows.first()
                .and_then(|r| r.get(0))
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(Self::error)
        };
        Ok((count(&nodes)?, count(&edges)?))
    }
    /// Read-only attestation: never repairs, publishes or moves a projection.
    pub(crate) fn verify_snapshot(&self, s: &Snapshot, hash: &str) -> Result<(), NexoraError> {
        let start = Instant::now();
        let scope = Self::scope(&s.tenant_id, &s.repository_id, &s.id)?;
        let rows = self.client.execute(
            "MATCH (s:NexoraSnapshot {scope:$scope}) RETURN s.tenant,s.repository,s.snapshot,s.payload_hash,s.state,s.query_version,s.node_count,s.edge_count",
            json!({"scope":scope}), Self::remaining(start)?,
        )?;
        let [row] = rows.as_slice() else {
            return Err(Self::error());
        };
        if row.get(0).and_then(Value::as_str) != Some(&s.tenant_id.0)
            || row.get(1).and_then(Value::as_str) != Some(&s.repository_id.0)
            || row.get(2).and_then(Value::as_str) != Some(&s.id.0)
            || row.get(3).and_then(Value::as_str) != Some(hash)
            || row.get(4).and_then(Value::as_str) != Some("ready")
            || row.get(5).and_then(Value::as_i64) != Some(i64::from(crate::QuerySnapshot::VERSION))
            || row.get(6).and_then(Value::as_u64) != Some(s.graph.nodes.len() as u64)
            || row.get(7).and_then(Value::as_u64) != Some(s.graph.edges.len() as u64)
            || self.counts(&scope, start)? != (s.graph.nodes.len(), s.graph.edges.len())
        {
            return Err(Self::error());
        }
        let edges = self.client.execute(
            "MATCH ()-[e:NEXORA_EDGE {scope:$scope}]->() RETURN count(e)",
            json!({"scope":scope}),
            Self::remaining(start)?,
        )?;
        if edges
            .first()
            .and_then(|row| row.get(0))
            .and_then(Value::as_u64)
            != Some(s.graph.edges.len() as u64)
        {
            return Err(Self::error());
        }
        self.verify_batches(&scope, s.graph.nodes.iter().enumerate().map(|(ordinal,node)| Self::node_row(ordinal,node)),
            "UNWIND $rows AS row OPTIONAL MATCH (n:NexoraNode {scope:$scope,node_id:row.id}) WITH row,collect(n) AS matches RETURN count(*),sum(CASE WHEN size(matches)=1 AND all(n IN matches WHERE n.name=row.name AND n.name_hash=row.name_hash AND n.ordinal=row.ordinal AND n.kind=row.kind AND n.payload_hash=row.payload_hash AND n.payload=row.payload) THEN 1 ELSE 0 END)", start)?;
        self.verify_batches(&scope, s.graph.edges.iter().enumerate().map(|(ordinal,edge)| Self::edge_row(ordinal,edge)),
            "UNWIND $rows AS row OPTIONAL MATCH (:NexoraNode {scope:$scope,node_id:row.source})-[e:NEXORA_EDGE {scope:$scope,edge_id:row.id}]->(:NexoraNode {scope:$scope,node_id:row.target}) WITH row,collect(e) AS matches RETURN count(*),sum(CASE WHEN size(matches)=1 AND all(e IN matches WHERE e.kind=row.kind AND e.resolution=row.resolution AND e.ordinal=row.ordinal AND e.payload_hash=row.payload_hash AND e.payload=row.payload) THEN 1 ELSE 0 END)", start)?;
        Ok(())
    }
    fn node_row(ordinal: usize, node: &nexora_graph::CodeNode) -> Result<Value, NexoraError> {
        let payload =
            String::from_utf8(BoundedJson::encode(node, 8_388_608)?).map_err(|_| Self::error())?;
        Ok(
            json!({"id":node.id.0,"name":node.name,"name_hash":format!("{:x}",Sha256::digest(node.name.as_bytes())),"ordinal":ordinal,"kind":node.kind,"payload_hash":format!("{:x}",Sha256::digest(payload.as_bytes())),"payload":payload}),
        )
    }
    fn edge_row(ordinal: usize, edge: &nexora_graph::CodeEdge) -> Result<Value, NexoraError> {
        let payload =
            String::from_utf8(BoundedJson::encode(edge, 8_388_608)?).map_err(|_| Self::error())?;
        Ok(
            json!({"id":edge.id.0,"source":edge.source.0,"target":edge.target.0,"kind":edge.kind,"resolution":edge.resolution,"ordinal":ordinal,"payload_hash":format!("{:x}",Sha256::digest(payload.as_bytes())),"payload":payload}),
        )
    }
    fn verify_batches(
        &self,
        scope: &str,
        records: impl IntoIterator<Item = Result<Value, NexoraError>>,
        statement: &str,
        start: Instant,
    ) -> Result<(), NexoraError> {
        let mut rows = Vec::new();
        let mut weight = 0usize;
        for row in records {
            let row = row?;
            let size = BoundedJson::encode(&row, 8_388_608)?.len();
            if !rows.is_empty() && (rows.len() == 512 || weight.saturating_add(size) > 8_388_608) {
                self.verify_batch(scope, &rows, statement, start)?;
                rows.clear();
                weight = 0;
            }
            rows.push(row);
            weight += size;
        }
        if !rows.is_empty() {
            self.verify_batch(scope, &rows, statement, start)?;
        }
        Ok(())
    }
    fn verify_batch(
        &self,
        scope: &str,
        records: &[Value],
        statement: &str,
        start: Instant,
    ) -> Result<(), NexoraError> {
        let rows = self.client.execute(
            statement,
            json!({"scope":scope,"rows":records}),
            Self::remaining(start)?,
        )?;
        let [row] = rows.as_slice() else {
            return Err(Self::error());
        };
        let expected = Some(records.len() as u64);
        if row.get(0).and_then(Value::as_u64) != expected
            || row.get(1).and_then(Value::as_u64) != expected
        {
            return Err(Self::error());
        }
        Ok(())
    }
    // Publication is batched and renewable: repository size must not impose a total
    // wall-clock deadline. Each database request remains bounded independently.
    pub fn project(&self, s: &Snapshot, hash: &str) -> Result<(), NexoraError> {
        let scope = Self::scope(&s.tenant_id, &s.repository_id, &s.id)?;
        for query in [
            "CREATE CONSTRAINT nexora_snapshot_scope IF NOT EXISTS FOR (s:NexoraSnapshot) REQUIRE s.scope IS UNIQUE",
            "CREATE CONSTRAINT nexora_node_identity IF NOT EXISTS FOR (n:NexoraNode) REQUIRE (n.scope,n.node_id) IS UNIQUE",
            "DROP INDEX nexora_node_name IF EXISTS",
            "CREATE INDEX nexora_node_name_hash IF NOT EXISTS FOR (n:NexoraNode) ON (n.scope,n.name_hash)",
            "CREATE INDEX nexora_node_scope IF NOT EXISTS FOR (n:NexoraNode) ON (n.scope)",
            "CREATE INDEX nexora_edge_scope IF NOT EXISTS FOR ()-[r:NEXORA_EDGE]-() ON (r.scope)",
        ] {
            self.client
                .execute(query, json!({}), Duration::from_secs(30))?;
        }
        let existing = self.client.execute(
            "MATCH (s:NexoraSnapshot {scope:$scope}) RETURN s.payload_hash,s.state,s.query_version",
            json!({"scope":scope}),
            Duration::from_secs(30),
        )?;
        if let Some(row) = existing.first() {
            if row.get(0).and_then(Value::as_str) != Some(hash) {
                return Err(Self::error());
            }
            if row.get(1).and_then(Value::as_str) == Some("ready")
                && row.get(2).and_then(Value::as_i64)
                    == Some(i64::from(crate::QuerySnapshot::VERSION))
                && self.counts(&scope, Instant::now())?
                    == (s.graph.nodes.len(), s.graph.edges.len())
            {
                return Ok(());
            }
        }
        self.client.execute("MERGE (s:NexoraSnapshot {scope:$scope}) SET s.tenant=$tenant,s.repository=$repository,s.snapshot=$snapshot,s.payload_hash=$hash,s.state='staging'",json!({"scope":scope,"tenant":s.tenant_id.0,"repository":s.repository_id.0,"snapshot":s.id.0,"hash":hash}),Duration::from_secs(30))?;
        let mut rows = Vec::new();
        let mut weight = 0usize;
        for (ordinal, node) in s.graph.nodes.iter().enumerate() {
            let row = Self::node_row(ordinal, node)?;
            let size = BoundedJson::encode(&row, 8_388_608)?.len();
            if !rows.is_empty() && (rows.len() == 512 || weight.saturating_add(size) > 8_388_608) {
                self.nodes(&scope, &rows)?;
                rows.clear();
                weight = 0;
            }
            rows.push(row);
            weight += size;
        }
        if !rows.is_empty() {
            self.nodes(&scope, &rows)?;
        }
        rows.clear();
        weight = 0;
        for (ordinal, edge) in s.graph.edges.iter().enumerate() {
            let row = Self::edge_row(ordinal, edge)?;
            let size = BoundedJson::encode(&row, 8_388_608)?.len();
            if !rows.is_empty() && (rows.len() == 512 || weight.saturating_add(size) > 8_388_608) {
                self.edges(&scope, &rows)?;
                rows.clear();
                weight = 0;
            }
            rows.push(row);
            weight += size;
        }
        if !rows.is_empty() {
            self.edges(&scope, &rows)?;
        }
        if self.counts(&scope, Instant::now())? != (s.graph.nodes.len(), s.graph.edges.len()) {
            return Err(Self::error());
        }
        self.client.execute("MATCH (s:NexoraSnapshot {scope:$scope,payload_hash:$hash}) SET s.state='ready',s.node_count=$nodes,s.edge_count=$edges,s.query_version=$version",json!({"scope":scope,"hash":hash,"nodes":s.graph.nodes.len(),"edges":s.graph.edges.len(),"version":crate::QuerySnapshot::VERSION}),Duration::from_secs(30))?;
        Ok(())
    }
    fn nodes(&self, scope: &str, rows: &[Value]) -> Result<(), NexoraError> {
        self.client.execute("UNWIND $rows AS row MERGE (n:NexoraNode {scope:$scope,node_id:row.id}) SET n.name=row.name,n.name_hash=row.name_hash,n.ordinal=row.ordinal,n.kind=row.kind,n.payload_hash=row.payload_hash,n.payload=row.payload",json!({"scope":scope,"rows":rows}),Duration::from_secs(30))?;
        Ok(())
    }
    fn edges(&self, scope: &str, rows: &[Value]) -> Result<(), NexoraError> {
        self.client.execute("UNWIND $rows AS row MATCH (a:NexoraNode {scope:$scope,node_id:row.source}),(b:NexoraNode {scope:$scope,node_id:row.target}) MERGE (a)-[r:NEXORA_EDGE {scope:$scope,edge_id:row.id}]->(b) SET r.kind=row.kind,r.resolution=row.resolution,r.ordinal=row.ordinal,r.payload_hash=row.payload_hash,r.payload=row.payload",json!({"scope":scope,"rows":rows}),Duration::from_secs(30))?;
        Ok(())
    }
    pub fn traverse(
        &self,
        s: &Snapshot,
        hash: &str,
        node: &NodeId,
        depth: usize,
        limit: usize,
    ) -> Result<GraphTraversal, NexoraError> {
        let scope = Self::scope(&s.tenant_id, &s.repository_id, &s.id)?;
        self.traverse_scope(&scope, hash, node, depth, limit)
    }
    pub(crate) fn traverse_scope(
        &self,
        scope: &str,
        hash: &str,
        node: &NodeId,
        depth: usize,
        limit: usize,
    ) -> Result<GraphTraversal, NexoraError> {
        if depth > 32 || limit == 0 || limit > 1000 {
            return Err(NexoraError::Limit(
                "graph depth<=32 and limit1..1000 required".into(),
            ));
        }
        let started = Instant::now();
        let state=self.client.execute("MATCH (s:NexoraSnapshot {scope:$scope,state:'ready',payload_hash:$hash}) MATCH (n:NexoraNode {scope:$scope,node_id:$node}) RETURN n.node_id",json!({"scope":scope,"hash":hash,"node":node.0}),Self::remaining(started)?)?;
        if state.is_empty() {
            return Err(Self::error());
        }
        let mut nodes = BTreeSet::from([node.0.clone()]);
        let mut edges = BTreeSet::new();
        let mut frontier = vec![node.0.clone()];
        let mut truncated = false;
        for _ in 0..depth {
            let mut next = Vec::new();
            let mut after = String::new();
            loop {
                let rows=self.client.execute("MATCH (a:NexoraNode {scope:$scope})-[r:NEXORA_EDGE {scope:$scope}]->(b:NexoraNode {scope:$scope}) WHERE a.node_id IN $frontier AND r.edge_id>$after RETURN r.edge_id,b.node_id ORDER BY r.edge_id LIMIT 512",json!({"scope":scope,"frontier":frontier,"after":after}),Self::remaining(started)?)?;
                for row in &rows {
                    let edge = row.get(0).and_then(Value::as_str).ok_or_else(Self::error)?;
                    let target = row.get(1).and_then(Value::as_str).ok_or_else(Self::error)?;
                    after = edge.into();
                    if edges.len() == 10_000 || (!nodes.contains(target) && nodes.len() == limit) {
                        truncated = true;
                        break;
                    }
                    edges.insert(edge.into());
                    if nodes.insert(target.into()) {
                        next.push(target.into());
                    }
                }
                if truncated || rows.len() < 512 {
                    break;
                }
            }
            frontier = next;
            if frontier.is_empty() || truncated {
                break;
            }
        }
        if !frontier.is_empty() {
            truncated = true;
        }
        Ok(GraphTraversal {
            node_ids: nodes.into_iter().collect(),
            edge_ids: edges.into_iter().collect(),
            truncated,
        })
    }
}
