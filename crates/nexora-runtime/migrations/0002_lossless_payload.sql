-- Preserve the canonical Snapshot JSON bytes, including escaped U+0000 in source.
DO $$ DECLARE stored_type text; BEGIN
 SELECT data_type INTO stored_type FROM information_schema.columns
 WHERE table_schema='nexora_runtime' AND table_name='snapshots' AND column_name='payload';
 IF stored_type='jsonb' THEN
   ALTER TABLE nexora_runtime.snapshots ALTER COLUMN payload TYPE bytea USING convert_to(payload::text,'UTF8');
 ELSIF stored_type IS DISTINCT FROM 'bytea' THEN
   RAISE EXCEPTION 'Unsupported snapshot payload type';
 END IF;
END $$;
DELETE FROM nexora_runtime.schema_versions WHERE version=1;
INSERT INTO nexora_runtime.schema_versions VALUES(2) ON CONFLICT DO NOTHING;
