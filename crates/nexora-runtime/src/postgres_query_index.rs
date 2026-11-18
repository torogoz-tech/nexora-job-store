use crate::{KnowledgeQuery, QuerySnapshot, bounded_json::BoundedJson};
use nexora_core::{NexoraError, RepositoryId, SnapshotId, TenantId};
use nexora_storage::Snapshot;
use postgres::{Client, binary_copy::BinaryCopyInWriter, types::Type};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) struct PostgresQueryIndex;
impl PostgresQueryIndex {
    fn error() -> NexoraError {
        NexoraError::Storage(
            "selective PostgreSQL query failed; prepare query indexes before reading".into(),
        )
    }
    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn verified(bytes: &[u8], hash: &str) -> Result<Value, NexoraError> {
        if bytes.len() > 1_048_576 || Self::digest(bytes) != hash {
            return Err(NexoraError::Storage(
                "query projection checksum mismatch".into(),
            ));
        }
        serde_json::from_slice(bytes).map_err(|_| Self::error())
    }
    pub fn project(client: &mut Client, s: &Snapshot, hash: &str) -> Result<(), NexoraError> {
        let existing=client.query_opt("SELECT version,payload_hash FROM nexora_runtime.query_snapshots WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0]).map_err(|_|Self::error())?;
        if let Some(row) = existing {
            if row.get::<_, String>(1) != hash {
                return Err(Self::error());
            }
            if row.get::<_, i32>(0) == QuerySnapshot::VERSION {
                return Ok(());
            }
        }
        let summary = json!({"nodes":s.graph.nodes.len(),"edges":s.graph.edges.len(),"files":s.sources.len(),"diagnostics":s.diagnostics.len(),"analysis_engine":s.analysis_engine,"rule_version":s.rule_version,"coverage":{"parsed_regions":s.graph.syntax_coverage.values().map(|c|c.parsed_regions).sum::<usize>(),"partial_regions":s.graph.syntax_coverage.values().map(|c|c.partial_regions).sum::<usize>(),"opaque_regions":s.graph.syntax_coverage.values().map(|c|c.opaque_regions).sum::<usize>(),"note":"Coverage is limited to the selected template; counts do not prove complete semantics."}});
        let summary = BoundedJson::encode(&summary, 65_536)?;
        let mut coverage: BTreeMap<&str, Vec<&nexora_core::SourceCoverage>> = BTreeMap::new();
        for c in &s.source_coverage {
            coverage.entry(&c.path).or_default().push(c);
        }
        let mut tx = client.transaction().map_err(|_| Self::error())?;
        // Bulk ingestion scales with repository size. Keep the normal read timeout
        // outside this transaction, but do not cancel a valid COPY after 15 seconds.
        tx.batch_execute("SET LOCAL statement_timeout = 0")
            .map_err(|_| Self::error())?;
        tx.execute("DELETE FROM nexora_runtime.query_lines WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0]).map_err(|_|Self::error())?;
        tx.execute("DELETE FROM nexora_runtime.query_files WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0]).map_err(|_|Self::error())?;
        {
            let sink=tx.copy_in("COPY nexora_runtime.query_files(tenant_id,repository_id,snapshot_id,path,total_lines,metadata,metadata_hash) FROM STDIN BINARY").map_err(|_|Self::error())?;
            let mut writer = BinaryCopyInWriter::new(
                sink,
                &[
                    Type::TEXT,
                    Type::TEXT,
                    Type::TEXT,
                    Type::TEXT,
                    Type::INT4,
                    Type::BYTEA,
                    Type::TEXT,
                ],
            );
            for (path, source) in &s.sources {
                let lines = i32::try_from(source.lines().count()).map_err(|_| Self::error())?;
                let metadata = BoundedJson::encode(
                    &json!({"file":{"path":path,"bytes":source.len(),"syntax_coverage":s.graph.syntax_coverage.get(path),"source_coverage":coverage.get(path.as_str()).cloned().unwrap_or_default()},"total_lines":lines}),
                    1_048_576,
                )?;
                writer
                    .write(&[
                        &s.tenant_id.0,
                        &s.repository_id.0,
                        &s.id.0,
                        path,
                        &lines,
                        &metadata,
                        &Self::digest(&metadata),
                    ])
                    .map_err(|_| Self::error())?;
            }
            writer.finish().map_err(|_| Self::error())?;
        }
        {
            let sink=tx.copy_in("COPY nexora_runtime.query_lines(tenant_id,repository_id,snapshot_id,path,line_number,content,content_hash,search_text) FROM STDIN BINARY").map_err(|_|Self::error())?;
            let mut writer = BinaryCopyInWriter::new(
                sink,
                &[
                    Type::TEXT,
                    Type::TEXT,
                    Type::TEXT,
                    Type::TEXT,
                    Type::INT4,
                    Type::BYTEA,
                    Type::TEXT,
                    Type::TEXT,
                ],
            );
            for (path, source) in &s.sources {
                for (i, line) in source.lines().enumerate() {
                    let number = i32::try_from(i + 1).map_err(|_| Self::error())?;
                    let bytes = line.as_bytes();
                    let search = line.replace('\0', "\u{fffd}");
                    writer
                        .write(&[
                            &s.tenant_id.0,
                            &s.repository_id.0,
                            &s.id.0,
                            path,
                            &number,
                            &bytes,
                            &crate::source_line_checksum::SourceLineChecksum::calculate(
                                &s.tenant_id.0,
                                &s.repository_id.0,
                                &s.id.0,
                                path,
                                number as usize,
                                bytes,
                            ),
                            &search,
                        ])
                        .map_err(|_| Self::error())?;
                }
            }
            writer.finish().map_err(|_| Self::error())?;
        }
        tx.execute("INSERT INTO nexora_runtime.query_snapshots(tenant_id,repository_id,snapshot_id,payload_hash,version,summary,summary_hash) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(tenant_id,repository_id,snapshot_id) DO UPDATE SET version=EXCLUDED.version,summary=EXCLUDED.summary,summary_hash=EXCLUDED.summary_hash",&[&s.tenant_id.0,&s.repository_id.0,&s.id.0,&hash,&QuerySnapshot::VERSION,&summary,&Self::digest(&summary)]).map_err(|_|Self::error())?;
        tx.commit().map_err(|_| Self::error())
    }
    pub fn head(
        client: &mut Client,
        t: &TenantId,
        r: &RepositoryId,
        id: Option<&SnapshotId>,
    ) -> Result<QuerySnapshot, NexoraError> {
        let id = id.map(|v| v.0.as_str());
        let row=client.query_typed_opt("SELECT s.snapshot_id,s.payload_hash,q.version,q.summary,q.summary_hash FROM nexora_runtime.repositories r JOIN nexora_runtime.snapshots s ON s.tenant_id=r.tenant_id AND s.repository_id=r.repository_id LEFT JOIN nexora_runtime.query_snapshots q ON q.tenant_id=s.tenant_id AND q.repository_id=s.repository_id AND q.snapshot_id=s.snapshot_id AND q.payload_hash=s.payload_hash WHERE r.tenant_id=$1 AND r.repository_id=$2 AND s.state='ready' AND s.snapshot_id=COALESCE($3,r.current_snapshot_id)",&[(&t.0, Type::TEXT),(&r.0, Type::TEXT),(&id, Type::TEXT)]).map_err(|_|Self::error())?.ok_or_else(Self::error)?;
        if row.get::<_, Option<i32>>(2) != Some(QuerySnapshot::VERSION) {
            return Err(Self::error());
        }
        let summary: Vec<u8> = row.get(3);
        let checksum: String = row.get(4);
        Ok(QuerySnapshot {
            tenant_id: t.clone(),
            repository_id: r.clone(),
            id: SnapshotId(row.get(0)),
            payload_hash: row.get(1),
            summary: Self::verified(&summary, &checksum)?,
        })
    }
    pub fn query(
        client: &mut impl postgres::GenericClient,
        head: &QuerySnapshot,
        query: &KnowledgeQuery,
    ) -> Result<Value, NexoraError> {
        let t = &head.tenant_id.0;
        let r = &head.repository_id.0;
        let id = &head.id.0;
        match query {
            KnowledgeQuery::Overview { offset, limit } => {
                let rows=client.query("SELECT metadata,metadata_hash FROM nexora_runtime.query_files WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 ORDER BY path COLLATE \"C\" OFFSET $4 LIMIT $5",&[t,r,id,&(*offset as i64),&(*limit as i64)]).map_err(|_|Self::error())?;
                let items = rows
                    .iter()
                    .map(|row| {
                        let metadata =
                            Self::verified(&row.get::<_, Vec<u8>>(0), &row.get::<_, String>(1))?;
                        metadata.get("file").cloned().ok_or_else(Self::error)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let total = head.summary["files"].as_u64().ok_or_else(Self::error)? as usize;
                let mut value = head.page(items, total, *offset);
                value
                    .as_object_mut()
                    .ok_or_else(Self::error)?
                    .extend(head.summary.as_object().ok_or_else(Self::error)?.clone());
                Ok(value)
            }
            KnowledgeQuery::Context {
                path,
                start_line,
                line_count,
            } => {
                let file=client.query_opt("SELECT total_lines,metadata,metadata_hash FROM nexora_runtime.query_files WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND path=$4",&[t,r,id,path]).map_err(|_|Self::error())?.ok_or_else(Self::error)?;
                let total = file.get::<_, i32>(0) as usize;
                let metadata =
                    Self::verified(&file.get::<_, Vec<u8>>(1), &file.get::<_, String>(2))?;
                if metadata["total_lines"].as_u64() != Some(total as u64) {
                    return Err(Self::error());
                }
                let rows=client.query("SELECT line_number,content,content_hash FROM nexora_runtime.query_lines WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND path=$4 AND line_number >= $5 AND line_number < $6 ORDER BY line_number",&[t,r,id,path,&(*start_line as i32),&((*start_line+*line_count) as i32)]).map_err(|_|Self::error())?;
                if rows.len() != (*line_count).min(total.saturating_sub(start_line - 1))
                    || rows
                        .iter()
                        .enumerate()
                        .any(|(i, row)| row.get::<_, i32>(0) as usize != start_line + i)
                {
                    return Err(Self::error());
                }
                let items = rows
                    .iter()
                    .map(|row| {
                        Self::line(
                            head,
                            path,
                            row.get::<_, i32>(0) as usize,
                            &row.get::<_, Vec<u8>>(1),
                            &row.get::<_, String>(2),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let end = (start_line - 1).saturating_add(items.len());
                let mut value = head.identity();
                value["total_lines"] = json!(total);
                value["next_line"] = if end < total {
                    json!(end + 1)
                } else {
                    Value::Null
                };
                value["truncated"] =
                    json!(end < total || items.iter().any(|v| v["text_truncated"] == true));
                value["items"] = json!(items);
                Ok(value)
            }
            KnowledgeQuery::Search {
                query,
                offset,
                limit,
            } => {
                let needle = query.as_bytes();
                let normalized = query.replace('\0', "\u{fffd}");
                let mut pattern = String::from("%");
                for c in normalized.chars() {
                    if matches!(c, '%' | '_' | '\\') {
                        pattern.push('\\');
                    }
                    pattern.push(c);
                }
                pattern.push('%');
                // Candidate normalization is a superset; byte verification precedes count and pagination.
                let total=client.query_one("SELECT count(*) FROM nexora_runtime.query_lines WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND search_text LIKE $4 ESCAPE E'\\\\' AND position($5::bytea IN content)>0",&[t,r,id,&pattern,&needle]).map_err(|_|Self::error())?.get::<_,i64>(0) as usize;
                let rows=client.query("SELECT path,line_number,content,content_hash FROM nexora_runtime.query_lines WHERE tenant_id=$1 AND repository_id=$2 AND snapshot_id=$3 AND search_text LIKE $4 ESCAPE E'\\\\' AND position($5::bytea IN content)>0 ORDER BY path COLLATE \"C\",line_number OFFSET $6 LIMIT $7",&[t,r,id,&pattern,&needle,&(*offset as i64),&(*limit as i64)]).map_err(|_|Self::error())?;
                let items = rows
                    .iter()
                    .map(|row| {
                        Self::line(
                            head,
                            &row.get::<_, String>(0),
                            row.get::<_, i32>(1) as usize,
                            &row.get::<_, Vec<u8>>(2),
                            &row.get::<_, String>(3),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(head.page(items, total, *offset))
            }
            _ => Err(Self::error()),
        }
    }
    fn line(
        head: &QuerySnapshot,
        path: &str,
        number: usize,
        bytes: &[u8],
        hash: &str,
    ) -> Result<Value, NexoraError> {
        if bytes.len() > 1_048_576
            || crate::source_line_checksum::SourceLineChecksum::calculate(
                &head.tenant_id.0,
                &head.repository_id.0,
                &head.id.0,
                path,
                number,
                bytes,
            ) != hash
        {
            return Err(NexoraError::Storage("source line checksum mismatch".into()));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Self::error())?;
        let mut end = text.len().min(8192);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(json!({"path":path,"line":number,"text":&text[..end],"text_truncated":end<text.len()}))
    }
}
