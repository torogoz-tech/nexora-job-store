use crate::{KnowledgeCommand, QuerySnapshot, bounded_json::BoundedJson};
use nexora_core::NexoraError;
use postgres::{Client, Row};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(crate) struct PostgresKnowledge;
impl PostgresKnowledge {
    fn error() -> NexoraError {
        NexoraError::Storage("knowledge revision unavailable, corrupted, conflicting request or stale expected_revision".into())
    }
    pub fn hash(value: &impl serde::Serialize) -> Result<String, NexoraError> {
        Ok(format!(
            "{:x}",
            Sha256::digest(BoundedJson::encode(value, 262_144)?)
        ))
    }
    fn decode(row: &Row, head: &QuerySnapshot) -> Result<Value, NexoraError> {
        let bytes: Vec<u8> = row.get("payload");
        if bytes.len() > 262_144
            || Self::hash(&(&head.tenant_id.0, &head.repository_id.0, &bytes))?
                != row.get::<_, String>("payload_hash")
        {
            return Err(Self::error());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Self::error())?;
        if value["id"] != row.get::<_, String>("knowledge_id")
            || value["revision"].as_i64() != Some(i64::from(row.get::<_, i32>("revision")))
            || value["snapshot"] != row.get::<_, String>("snapshot_id")
            || value["state"] != row.get::<_, String>("state")
            || value["draft"]["source"].as_str()
                != row.get::<_, Option<String>>("source_node").as_deref()
            || value["draft"]["target"].as_str()
                != row.get::<_, Option<String>>("target_node").as_deref()
            || value["request_id"] != row.get::<_, String>("request_id")
        {
            return Err(Self::error());
        }
        Ok(value)
    }
    pub fn receipt(
        client: &mut impl postgres::GenericClient,
        head: &QuerySnapshot,
        request: &str,
        hash: &str,
    ) -> Result<Option<Value>, NexoraError> {
        let row=client.query_opt("SELECT * FROM nexora_runtime.knowledge_revisions WHERE tenant_id=$1 AND repository_id=$2 AND request_id=$3",&[&head.tenant_id.0,&head.repository_id.0,&request]).map_err(|_|Self::error())?;
        row.map(|row| {
            if row.get::<_, String>("request_hash") != hash {
                return Err(Self::error());
            }
            Self::decode(&row, head)
        })
        .transpose()
    }
    pub fn latest(
        client: &mut impl postgres::GenericClient,
        head: &QuerySnapshot,
        id: &str,
    ) -> Result<Value, NexoraError> {
        let row=client.query_opt("SELECT r.* FROM nexora_runtime.knowledge_heads h JOIN nexora_runtime.knowledge_revisions r USING(tenant_id,repository_id,knowledge_id,revision) WHERE h.tenant_id=$1 AND h.repository_id=$2 AND h.knowledge_id=$3",&[&head.tenant_id.0,&head.repository_id.0,&id]).map_err(|_|Self::error())?.ok_or_else(Self::error)?;
        Self::decode(&row, head)
    }
    pub fn append(
        client: &mut impl postgres::GenericClient,
        head: &QuerySnapshot,
        mut record: Value,
        expected: u32,
        request_hash: &str,
    ) -> Result<Value, NexoraError> {
        let mut tx = client.transaction().map_err(|_| Self::error())?;
        let id = record["id"].as_str().ok_or_else(Self::error)?.to_owned();
        let next = i32::try_from(expected + 1).map_err(|_| Self::error())?;
        let count=if expected==0 {
            tx.execute("INSERT INTO nexora_runtime.knowledge_heads(tenant_id,repository_id,knowledge_id,revision) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",&[&head.tenant_id.0,&head.repository_id.0,&id,&next])
        }else{
            tx.execute("UPDATE nexora_runtime.knowledge_heads SET revision=$4 WHERE tenant_id=$1 AND repository_id=$2 AND knowledge_id=$3 AND revision=$5",&[&head.tenant_id.0,&head.repository_id.0,&id,&next,&(expected as i32)])
        }.map_err(|_|Self::error())?;
        if count != 1 {
            return Err(Self::error());
        }
        record["created_at"] = json!(
            tx.query_one("SELECT clock_timestamp()::text", &[])
                .map_err(|_| Self::error())?
                .get::<_, String>(0)
        );
        let bytes = BoundedJson::encode(&record, 65_536)?;
        let hash = Self::hash(&(&head.tenant_id.0, &head.repository_id.0, &bytes))?;
        tx.execute("INSERT INTO nexora_runtime.knowledge_revisions(tenant_id,repository_id,knowledge_id,revision,snapshot_id,state,source_node,target_node,request_id,request_hash,payload,payload_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",&[&head.tenant_id.0,&head.repository_id.0,&id,&next,&record["snapshot"].as_str().ok_or_else(Self::error)?,&record["state"].as_str().ok_or_else(Self::error)?,&record["draft"]["source"].as_str(),&record["draft"]["target"].as_str(),&record["request_id"].as_str().ok_or_else(Self::error)?,&request_hash,&bytes,&hash]).map_err(|_|Self::error())?;
        tx.commit().map_err(|_| Self::error())?;
        Ok(record)
    }
    pub fn decorate(mut record: Value, head: &QuerySnapshot, latest_revision: u64) -> Value {
        let current = record["snapshot"] == head.id.0;
        record["validity"] = json!(if current { "current" } else { "needs_review" });
        let is_latest = record["revision"].as_u64() == Some(latest_revision);
        record["is_latest"] = json!(is_latest);
        record["effective"] = json!(current && is_latest && record["state"] == "accepted");
        record
    }
    pub fn read(
        client: &mut Client,
        head: &QuerySnapshot,
        command: &KnowledgeCommand,
    ) -> Result<Value, NexoraError> {
        let mut tx = client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .map_err(|_| Self::error())?;
        let mut result = head.identity();
        match command {
            KnowledgeCommand::Get { id, offset, limit } => {
                let row=tx.query_opt("SELECT r.* FROM nexora_runtime.knowledge_heads h JOIN nexora_runtime.knowledge_revisions r USING(tenant_id,repository_id,knowledge_id,revision) WHERE h.tenant_id=$1 AND h.repository_id=$2 AND h.knowledge_id=$3",&[&head.tenant_id.0,&head.repository_id.0,&id]).map_err(|_|Self::error())?.ok_or_else(Self::error)?;
                let record = Self::decode(&row, head)?;
                let total = record["revision"].as_u64().ok_or_else(Self::error)? as usize;
                let rows=tx.query("SELECT * FROM nexora_runtime.knowledge_revisions WHERE tenant_id=$1 AND repository_id=$2 AND knowledge_id=$3 ORDER BY revision OFFSET $4 LIMIT $5",&[&head.tenant_id.0,&head.repository_id.0,&id,&(*offset as i64),&(*limit as i64)]).map_err(|_|Self::error())?;
                let history = rows
                    .iter()
                    .map(|r| Self::decode(r, head).map(|v| Self::decorate(v, head, total as u64)))
                    .collect::<Result<Vec<_>, _>>()?;
                result["contribution"] = Self::decorate(record, head, total as u64);
                result["history_total"] = json!(total);
                let next = offset + history.len();
                result["next_offset"] = if next < total {
                    json!(next)
                } else {
                    Value::Null
                };
                result["history"] = json!(history);
            }
            KnowledgeCommand::List {
                status,
                node,
                offset,
                limit,
            } => {
                let status = status.as_deref();
                let node = node.as_deref();
                let predicate = "FROM nexora_runtime.knowledge_heads h JOIN nexora_runtime.knowledge_revisions r USING(tenant_id,repository_id,knowledge_id,revision) WHERE h.tenant_id=$1 AND h.repository_id=$2 AND ($3::text IS NULL OR ($3='needs_review' AND r.snapshot_id<>$4 AND r.state IN ('proposed','accepted')) OR ($3<>'needs_review' AND r.state=$3)) AND ($5::text IS NULL OR r.source_node=$5 OR r.target_node=$5)";
                let total = tx
                    .query_one(
                        &format!("SELECT count(*) {predicate}"),
                        &[
                            &head.tenant_id.0,
                            &head.repository_id.0,
                            &status,
                            &head.id.0,
                            &node,
                        ],
                    )
                    .map_err(|_| Self::error())?
                    .get::<_, i64>(0) as usize;
                let rows=tx.query(&format!("SELECT r.* {predicate} ORDER BY h.knowledge_id COLLATE \"C\" OFFSET $6 LIMIT $7"),&[&head.tenant_id.0,&head.repository_id.0,&status,&head.id.0,&node,&(*offset as i64),&(*limit as i64)]).map_err(|_|Self::error())?;
                let values = rows
                    .iter()
                    .map(|r| {
                        Self::decode(r, head).map(|v| {
                            let latest = v["revision"].as_u64().unwrap_or(0);
                            Self::decorate(v, head, latest)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(head.page(values, total, *offset));
            }
            _ => return Err(Self::error()),
        }
        Ok(result)
    }
}
