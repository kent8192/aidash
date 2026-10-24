-- Reverse only the extension created by this history. Borrowed infrastructure
-- remains available to its administrator after all application objects are gone.
DO $aidash_extension$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_extension
        WHERE extname = 'pg_jsonschema'
          AND obj_description(oid, 'pg_extension') = 'aidash:operations.0000_environment'
    ) THEN
        DROP EXTENSION pg_jsonschema;
    END IF;
END
$aidash_extension$;
