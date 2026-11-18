use crate::{KnowledgeQuery, Neo4jProjection, QuerySnapshot, neo4j_client::Neo4jClient};
use nexora_core::NexoraError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub(crate) struct Neo4jQueries<'a> {
    graph: &'a Neo4jProjection,
    client: &'a Neo4jClient,
}
impl<'a> Neo4jQueries<'a> {
    pub fn new(graph: &'a Neo4jProjection) -> Self {
        Self {
            graph,
            client: graph.client(),
        }
    }
    fn error() -> NexoraError {
        NexoraError::Storage("published graph query unavailable or checksum mismatch".into())
    }
    fn decode(pair: &Value) -> Result<Value, NexoraError> {
        let payload = pair
            .get(0)
            .and_then(Value::as_str)
            .ok_or_else(Self::error)?;
        let hash = pair
            .get(1)
            .and_then(Value::as_str)
            .ok_or_else(Self::error)?;
        if payload.len() > 8_388_608 || format!("{:x}", Sha256::digest(payload.as_bytes())) != hash
        {
            return Err(Self::error());
        }
        serde_json::from_str(payload).map_err(|_| Self::error())
    }
    pub fn node(&self, head: &QuerySnapshot, node: &str) -> Result<Value, NexoraError> {
        let scope = Neo4jProjection::scope(&head.tenant_id, &head.repository_id, &head.id)?;
        let rows=self.client.execute("MATCH (:NexoraSnapshot {scope:$scope,state:'ready',payload_hash:$hash,query_version:$version}) MATCH (n:NexoraNode {scope:$scope,node_id:$node}) RETURN [n.payload,n.payload_hash]",json!({"scope":scope,"hash":head.payload_hash,"version":QuerySnapshot::VERSION,"node":node}),Duration::from_secs(30))?;
        let value = Self::decode(
            rows.first()
                .and_then(|r| r.get(0))
                .ok_or_else(Self::error)?,
        )?;
        if value["id"] != node {
            return Err(Self::error());
        }
        Ok(value)
    }
    pub fn pending(
        &self,
        head: &QuerySnapshot,
        offset: usize,
        limit: usize,
    ) -> Result<Value, NexoraError> {
        let scope = Neo4jProjection::scope(&head.tenant_id, &head.repository_id, &head.id)?;
        let rows=self.client.execute("MATCH (s:NexoraSnapshot {scope:$scope,state:'ready',payload_hash:$hash,query_version:$version}) CALL { WITH s MATCH (:NexoraNode {scope:$scope})-[e:NEXORA_EDGE {scope:$scope}]->(:NexoraNode {scope:$scope}) WHERE e.resolution IN ['Unresolved','Ambiguous'] RETURN count(e) AS total } CALL { WITH s MATCH (:NexoraNode {scope:$scope})-[e:NEXORA_EDGE {scope:$scope}]->(:NexoraNode {scope:$scope}) WHERE e.resolution IN ['Unresolved','Ambiguous'] WITH e ORDER BY e.ordinal SKIP $offset LIMIT $limit RETURN collect([e.payload,e.payload_hash]) AS items } RETURN total,items",json!({"scope":scope,"hash":head.payload_hash,"version":QuerySnapshot::VERSION,"offset":offset,"limit":limit}),Duration::from_secs(30))?;
        let row = rows.first().ok_or_else(Self::error)?;
        let total = row.get(0).and_then(Value::as_u64).ok_or_else(Self::error)? as usize;
        let items = row
            .get(1)
            .and_then(Value::as_array)
            .ok_or_else(Self::error)?
            .iter()
            .map(Self::decode)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(head.page(items, total, offset))
    }
    pub fn query(
        &self,
        head: &QuerySnapshot,
        query: &KnowledgeQuery,
    ) -> Result<Value, NexoraError> {
        let scope = Neo4jProjection::scope(&head.tenant_id, &head.repository_id, &head.id)?;
        let mut parameters =
            json!({"scope":scope,"hash":head.payload_hash,"version":QuerySnapshot::VERSION});
        if let KnowledgeQuery::Traverse { node, depth, limit } = query {
            let traversal =
                self.graph
                    .traverse_scope(&scope, &head.payload_hash, node, *depth, *limit)?;
            parameters["nodes"] = json!(traversal.node_ids);
            parameters["edges"] = json!(traversal.edge_ids);
            let rows=self.client.execute("MATCH (s:NexoraSnapshot {scope:$scope,state:'ready',payload_hash:$hash,query_version:$version}) CALL { WITH s MATCH (n:NexoraNode {scope:$scope}) WHERE n.node_id IN $nodes WITH n ORDER BY n.ordinal RETURN collect([n.payload,n.payload_hash]) AS nodes } CALL { WITH s MATCH (:NexoraNode {scope:$scope})-[e:NEXORA_EDGE {scope:$scope}]->(:NexoraNode {scope:$scope}) WHERE e.edge_id IN $edges WITH e ORDER BY e.ordinal RETURN collect([e.payload,e.payload_hash]) AS edges } RETURN nodes,edges",parameters,Duration::from_secs(30))?;
            let row = rows.first().ok_or_else(Self::error)?;
            let decode = |index: usize| -> Result<Vec<Value>, NexoraError> {
                row.get(index)
                    .and_then(Value::as_array)
                    .ok_or_else(Self::error)?
                    .iter()
                    .map(Self::decode)
                    .collect()
            };
            let mut result = head.identity();
            result["nodes"] = json!(decode(0)?);
            result["edges"] = json!(decode(1)?);
            result["truncated"] = json!(traversal.truncated);
            return Ok(result);
        }
        let (offset, limit) = query.page();
        parameters["offset"] = json!(offset);
        parameters["limit"] = json!(limit);
        let guard = "MATCH (s:NexoraSnapshot {scope:$scope,state:'ready',payload_hash:$hash,query_version:$version}) ";
        let (start, pattern, predicate, entity) = match query {
            KnowledgeQuery::FindSymbol { name, .. } => {
                parameters["name"] = json!(name);
                parameters["name_hash"] = json!(format!("{:x}", Sha256::digest(name.as_bytes())));
                (
                    "",
                    "MATCH (n:NexoraNode {scope:$scope,name_hash:$name_hash})",
                    "n.name=$name AND NOT n.kind IN ['File','Repository']",
                    "n",
                )
            }
            KnowledgeQuery::References { node, .. } => {
                parameters["node"] = json!(node.0);
                (
                    "MATCH (start:NexoraNode {scope:$scope,node_id:$node}) ",
                    "MATCH (:NexoraNode {scope:$scope})-[e:NEXORA_EDGE {scope:$scope}]->(start)",
                    "NOT e.kind IN ['CONTAINS','DECLARES','INTERPRETS','SCOPE_BRIDGE']",
                    "e",
                )
            }
            KnowledgeQuery::Dependencies { node, .. } => {
                parameters["node"] = json!(node.0);
                (
                    "MATCH (start:NexoraNode {scope:$scope,node_id:$node}) ",
                    "MATCH (start)-[e:NEXORA_EDGE {scope:$scope}]->(:NexoraNode {scope:$scope})",
                    "e.kind IN ['IMPORTS','DEPENDS_ON']",
                    "e",
                )
            }
            _ => return Err(Self::error()),
        };
        let variables = if start.is_empty() { "s" } else { "s,start" };
        // Count only indexed matches; materialize payloads solely for the requested page.
        let statement = format!(
            "{guard}{start}CALL {{ WITH {variables} {pattern} WHERE {predicate} RETURN count({entity}) AS total }} CALL {{ WITH {variables} {pattern} WHERE {predicate} WITH {entity} ORDER BY {entity}.ordinal SKIP $offset LIMIT $limit RETURN collect([{entity}.payload,{entity}.payload_hash]) AS items }} RETURN total,items"
        );
        let rows = self
            .client
            .execute(&statement, parameters, Duration::from_secs(30))?;
        let row = rows.first().ok_or_else(Self::error)?;
        let total = row.get(0).and_then(Value::as_u64).ok_or_else(Self::error)? as usize;
        let items = row
            .get(1)
            .and_then(Value::as_array)
            .ok_or_else(Self::error)?
            .iter()
            .map(Self::decode)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(head.page(items, total, offset))
    }
}
