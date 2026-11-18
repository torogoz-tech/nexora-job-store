use crate::{
    KnowledgeCommand, KnowledgeDraft, Neo4jProjection, PostgresSnapshots, QuerySnapshot,
    knowledge_evidence::KnowledgeEvidence, postgres_knowledge::PostgresKnowledge,
    postgres_query_index::PostgresQueryIndex,
};
use nexora_core::{NexoraError, RepositoryId, SnapshotId, TenantId};
use serde_json::{Value, json};
pub(crate) struct KnowledgeService;
impl KnowledgeService {
    pub fn execute(
        postgres: &mut PostgresSnapshots,
        graph: &Neo4jProjection,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
        actor: &str,
        command: &KnowledgeCommand,
    ) -> Result<Value, NexoraError> {
        command.validate()?;
        let _lease = if command.is_write() {
            Some(postgres.lease(t, r)?)
        } else {
            None
        };
        let head = PostgresQueryIndex::head(postgres.client(), t, r, id)?;
        if !command.is_write() {
            return PostgresKnowledge::read(postgres.client(), &head, command);
        }
        Self::write(postgres.client(), graph, &head, id, actor, command)
    }
    pub(crate) fn write(
        client: &mut impl postgres::GenericClient,
        graph: &Neo4jProjection,
        head: &QuerySnapshot,
        id: Option<&SnapshotId>,
        actor: &str,
        command: &KnowledgeCommand,
    ) -> Result<Value, NexoraError> {
        command.validate()?;
        if !command.is_write() {
            return Err(NexoraError::Configuration(
                "knowledge mutation required".into(),
            ));
        }
        if actor.trim().is_empty() || actor.len() > 128 || actor.chars().any(char::is_control) {
            return Err(NexoraError::Configuration(
                "knowledge write actor required".into(),
            ));
        }
        let request = command.request_id().unwrap();
        let request_hash = PostgresKnowledge::hash(&(actor, id, command))?;
        if let Some(record) = PostgresKnowledge::receipt(client, head, request, &request_hash)? {
            let latest = PostgresKnowledge::latest(client, head, record["id"].as_str().unwrap())?;
            let latest_revision = latest["revision"]
                .as_u64()
                .ok_or_else(|| NexoraError::Storage("invalid knowledge revision".into()))?;
            return Ok(Self::response(head, record, latest_revision));
        }
        let mut record;
        let expected;
        match command {
            KnowledgeCommand::Contribute { draft, .. } => {
                let anchors = KnowledgeEvidence::verify(client, graph, head, draft)?;
                let knowledge_id = format!(
                    "knowledge:{}",
                    PostgresKnowledge::hash(&(
                        &head.tenant_id,
                        &head.repository_id,
                        actor,
                        request
                    ))?
                );
                expected = 0;
                record = json!({"id":knowledge_id,"revision":1,"snapshot":head.id.0,"snapshot_hash":head.payload_hash,"origin":"contributed","draft":draft,"verified_evidence":anchors,"state":"proposed","reason":Value::Null});
            }
            KnowledgeCommand::Revise {
                id,
                expected_revision,
                draft,
                ..
            } => {
                let previous = PostgresKnowledge::latest(client, head, id)?;
                Self::expected(&previous, *expected_revision)?;
                let anchors = KnowledgeEvidence::verify(client, graph, head, draft)?;
                expected = *expected_revision;
                record = json!({"id":id,"revision":expected+1,"snapshot":head.id.0,"snapshot_hash":head.payload_hash,"origin":"contributed","draft":draft,"verified_evidence":anchors,"state":"proposed","reason":Value::Null});
            }
            KnowledgeCommand::Review {
                id,
                expected_revision,
                action,
                reason,
                ..
            } => {
                record = PostgresKnowledge::latest(client, head, id)?;
                Self::expected(&record, *expected_revision)?;
                if action == "accept" {
                    if record["snapshot"] != head.id.0 || record["state"] == "withdrawn" {
                        return Err(NexoraError::Configuration(
                            "revise stale or withdrawn knowledge before acceptance".into(),
                        ));
                    }
                    let draft: KnowledgeDraft = serde_json::from_value(record["draft"].clone())
                        .map_err(|_| NexoraError::Storage("invalid stored draft".into()))?;
                    let evidence = KnowledgeEvidence::verify(client, graph, head, &draft)?;
                    if record["verified_evidence"] != evidence {
                        return Err(NexoraError::Storage("knowledge evidence changed".into()));
                    }
                }
                expected = *expected_revision;
                record["revision"] = json!(expected + 1);
                record["state"] = json!(match action.as_str() {
                    "accept" => "accepted",
                    "reject" => "rejected",
                    _ => "withdrawn",
                });
                record["reason"] = json!(reason);
            }
            _ => unreachable!(),
        }
        record["actor"] = json!(actor);
        record["request_id"] = json!(request);
        record["operation"] = json!(match command {
            KnowledgeCommand::Contribute { .. } => "contribute",
            KnowledgeCommand::Revise { .. } => "revise",
            _ => "review",
        });
        let record = PostgresKnowledge::append(client, head, record, expected, &request_hash)?;
        let latest_revision = record["revision"].as_u64().unwrap();
        Ok(Self::response(head, record, latest_revision))
    }
    fn expected(record: &Value, expected: u32) -> Result<(), NexoraError> {
        if record["revision"].as_u64() != Some(u64::from(expected)) {
            return Err(NexoraError::Configuration(
                "stale expected_revision; read current knowledge before editing".into(),
            ));
        }
        Ok(())
    }
    fn response(head: &QuerySnapshot, record: Value, latest_revision: u64) -> Value {
        let mut result = head.identity();
        result["contribution"] = PostgresKnowledge::decorate(record, head, latest_revision);
        result
    }
}
