-- Restore the 0019_deferred_exposure model constraint and installation guard.
-- Rollback precondition: no model Definition, Package or installation override
-- may carry the streaming keys. Pre-0020 binaries decode model configs with
-- deny_unknown_fields, and neither the restored registry constraint nor the
-- replaced trigger rechecks existing installation overrides. This read-only
-- guard refuses the rollback instead of leaving records no binary can read.
DO $$
DECLARE definition_count bigint; package_count bigint; installation_count bigint;
BEGIN
  SELECT count(*) INTO definition_count FROM registry
   WHERE kind = 'model' AND metadata->'config' ?| ARRAY['streaming','stream_stall_timeout_secs'];
  SELECT count(*) INTO package_count FROM packages
   WHERE manifest #>> '{entity,kind}' = 'model'
     AND manifest #> '{entity,config}' ?| ARRAY['streaming','stream_stall_timeout_secs'];
  SELECT count(*) INTO installation_count FROM installations
    JOIN registry ON registry.id = installations.id AND registry.version = installations.version
   WHERE registry.kind = 'model' AND installations.config ?| ARRAY['streaming','stream_stall_timeout_secs'];
  IF definition_count > 0 OR package_count > 0 OR installation_count > 0 THEN
    RAISE EXCEPTION 'registry 0020_model_streaming_config cannot be reversed: % Definitions, % Packages and % installation overrides use streaming or stream_stall_timeout_secs', definition_count, package_count, installation_count
      USING ERRCODE = '55000',
            HINT = 'Pre-0020 binaries cannot read these records; remove the streaming settings or restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_model_streaming;
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''request_timeout_secs''::text, ''streaming''::text, ''stream_stall_timeout_secs''::text, ''media_routes''::text', '''request_timeout_secs''::text, ''media_routes''::text');
  IF edited = definition THEN RAISE EXCEPTION 'Streaming rollback allowlist anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
-- Withdraw only the streaming checks from the CURRENT installation guard,
-- restoring the 0019_deferred_exposure function body.
DO $do$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_functiondef('public.guard_installation_config()'::regprocedure) INTO STRICT definition;
  edited := replace(definition, $old$(CASE WHEN (NEW.config->'stream_stall_timeout_secs') IS NULL OR (NEW.config->'stream_stall_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'stream_stall_timeout_secs') = 'number' AND (NEW.config->'stream_stall_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'stream_stall_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (NOT NEW.config ? 'streaming' OR jsonb_typeof(NEW.config->'streaming') IN ('boolean','null')) AND
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','streaming','stream_stall_timeout_secs','media_routes']::text[])$old$, $new$(NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','media_routes']::text[])$new$);
  IF edited = definition THEN RAISE EXCEPTION 'installation model override streaming addition missing'; END IF;
  EXECUTE edited;
END $do$;
