CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE TABLE IF NOT EXISTS nexora_runtime.query_snapshots(
 tenant_id text NOT NULL, repository_id text NOT NULL, snapshot_id text NOT NULL,
 payload_hash text NOT NULL, version integer NOT NULL,
 summary bytea NOT NULL, summary_hash text NOT NULL,
 PRIMARY KEY(tenant_id,repository_id,snapshot_id),
 FOREIGN KEY(tenant_id,repository_id,snapshot_id) REFERENCES nexora_runtime.snapshots(tenant_id,repository_id,snapshot_id)
);
CREATE TABLE IF NOT EXISTS nexora_runtime.query_files(
 tenant_id text NOT NULL, repository_id text NOT NULL, snapshot_id text NOT NULL,
 path text COLLATE "C" NOT NULL, total_lines integer NOT NULL,
 metadata bytea NOT NULL, metadata_hash text NOT NULL,
 PRIMARY KEY(tenant_id,repository_id,snapshot_id,path),
 FOREIGN KEY(tenant_id,repository_id,snapshot_id) REFERENCES nexora_runtime.snapshots(tenant_id,repository_id,snapshot_id)
);
CREATE TABLE IF NOT EXISTS nexora_runtime.query_lines(
 tenant_id text NOT NULL, repository_id text NOT NULL, snapshot_id text NOT NULL,
 path text COLLATE "C" NOT NULL, line_number integer NOT NULL,
 content bytea NOT NULL, content_hash text NOT NULL, search_text text NOT NULL,
 PRIMARY KEY(tenant_id,repository_id,snapshot_id,path,line_number),
 FOREIGN KEY(tenant_id,repository_id,snapshot_id,path) REFERENCES nexora_runtime.query_files(tenant_id,repository_id,snapshot_id,path)
);
CREATE INDEX IF NOT EXISTS query_lines_trigram ON nexora_runtime.query_lines USING gin(search_text gin_trgm_ops);
DELETE FROM nexora_runtime.schema_versions WHERE version=2;
INSERT INTO nexora_runtime.schema_versions VALUES(3) ON CONFLICT DO NOTHING;
