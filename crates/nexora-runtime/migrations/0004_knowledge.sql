CREATE TABLE IF NOT EXISTS nexora_runtime.knowledge_heads(
 tenant_id text NOT NULL, repository_id text NOT NULL, knowledge_id text NOT NULL,
 revision integer NOT NULL CHECK(revision>0),
 PRIMARY KEY(tenant_id,repository_id,knowledge_id),
 FOREIGN KEY(tenant_id,repository_id) REFERENCES nexora_runtime.repositories(tenant_id,repository_id)
);
CREATE TABLE IF NOT EXISTS nexora_runtime.knowledge_revisions(
 tenant_id text NOT NULL, repository_id text NOT NULL, knowledge_id text NOT NULL,
 revision integer NOT NULL CHECK(revision>0), snapshot_id text NOT NULL,
 state text NOT NULL CHECK(state IN ('proposed','accepted','rejected','withdrawn')),
 source_node text, target_node text, request_id text NOT NULL,
 request_hash text NOT NULL, payload bytea NOT NULL, payload_hash text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(tenant_id,repository_id,knowledge_id,revision),
 UNIQUE(tenant_id,repository_id,request_id),
 FOREIGN KEY(tenant_id,repository_id,knowledge_id) REFERENCES nexora_runtime.knowledge_heads(tenant_id,repository_id,knowledge_id),
 FOREIGN KEY(tenant_id,repository_id,snapshot_id) REFERENCES nexora_runtime.snapshots(tenant_id,repository_id,snapshot_id)
);
CREATE INDEX IF NOT EXISTS knowledge_revision_source ON nexora_runtime.knowledge_revisions(tenant_id,repository_id,source_node);
CREATE INDEX IF NOT EXISTS knowledge_revision_target ON nexora_runtime.knowledge_revisions(tenant_id,repository_id,target_node);
DELETE FROM nexora_runtime.schema_versions WHERE version=3;
INSERT INTO nexora_runtime.schema_versions VALUES(4) ON CONFLICT DO NOTHING;
