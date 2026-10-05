-- Native CreateExtension cannot record whether conditional creation owns an
-- extension. Mark only a new extension; preserve preprovisioned ownership.
DO $aidash_extension$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_jsonschema') THEN
        CREATE EXTENSION pg_jsonschema WITH SCHEMA public;
        COMMENT ON EXTENSION pg_jsonschema IS 'aidash:operations.0000_environment';
    END IF;
END
$aidash_extension$;
