CREATE SCHEMA IF NOT EXISTS nexora_runtime;
CREATE TABLE IF NOT EXISTS nexora_runtime.schema_versions(version integer PRIMARY KEY);
CREATE TABLE IF NOT EXISTS nexora_runtime.repositories(
 tenant_id text NOT NULL, repository_id text NOT NULL, current_snapshot_id text,
 PRIMARY KEY(tenant_id,repository_id)
);
CREATE TABLE IF NOT EXISTS nexora_runtime.snapshots(
 tenant_id text NOT NULL, repository_id text NOT NULL, snapshot_id text NOT NULL,
 payload bytea NOT NULL, payload_hash text NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','ready')),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(tenant_id,repository_id,snapshot_id),
 FOREIGN KEY(tenant_id,repository_id) REFERENCES nexora_runtime.repositories(tenant_id,repository_id)
);
DO $$ BEGIN
 IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname='runtime_current_snapshot_fk' AND conrelid='nexora_runtime.repositories'::regclass) THEN
 ALTER TABLE nexora_runtime.repositories ADD CONSTRAINT runtime_current_snapshot_fk
 FOREIGN KEY(tenant_id,repository_id,current_snapshot_id) REFERENCES nexora_runtime.snapshots(tenant_id,repository_id,snapshot_id) DEFERRABLE INITIALLY DEFERRED;
 END IF;
END $$;
