-- Model streaming transport settings: admit the optional `streaming` and
-- `stream_stall_timeout_secs` keys. PostgreSQL JSONB CHECK expressions,
-- catalog-driven constraint edits and function bodies have no typed Reinhardt
-- operation. DDL only; no application data is modified.
-- Extend only the allowlist of the CURRENT model constraint, read from the
-- catalog, so earlier additions (provider_credential, projection_versions,
-- cache_mode) survive.
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''request_timeout_secs''::text, ''media_routes''::text', '''request_timeout_secs''::text, ''streaming''::text, ''stream_stall_timeout_secs''::text, ''media_routes''::text');
  IF edited = definition THEN RAISE EXCEPTION 'Streaming allowlist anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
ALTER TABLE registry ADD CONSTRAINT registry_model_streaming CHECK (
  kind <> 'model' OR (
    (CASE WHEN metadata #> '{config,streaming}' IS NULL OR metadata #> '{config,streaming}' = 'null'::jsonb THEN true
      ELSE jsonb_typeof(metadata #> '{config,streaming}') = 'boolean' END)
    AND (CASE WHEN metadata #> '{config,stream_stall_timeout_secs}' IS NULL OR metadata #> '{config,stream_stall_timeout_secs}' = 'null'::jsonb THEN true
      WHEN jsonb_typeof(metadata #> '{config,stream_stall_timeout_secs}') = 'number' AND (metadata #> '{config,stream_stall_timeout_secs}')::text ~ '^[1-9][0-9]*$'
        THEN (metadata #> '{config,stream_stall_timeout_secs}')::text::numeric BETWEEN 1 AND 4294967295
      ELSE false END)
  )
);
-- Edit the CURRENT installation guard read from the catalog, so other
-- sections (such as the 0019_deferred_exposure Agent `exposure` key) survive.
DO $do$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_functiondef('public.guard_installation_config()'::regprocedure) INTO STRICT definition;
  edited := replace(definition, $old$(NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','media_routes']::text[])$old$, $new$(CASE WHEN (NEW.config->'stream_stall_timeout_secs') IS NULL OR (NEW.config->'stream_stall_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'stream_stall_timeout_secs') = 'number' AND (NEW.config->'stream_stall_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'stream_stall_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (NOT NEW.config ? 'streaming' OR jsonb_typeof(NEW.config->'streaming') IN ('boolean','null')) AND
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','streaming','stream_stall_timeout_secs','media_routes']::text[])$new$);
  IF edited = definition THEN RAISE EXCEPTION 'installation model override allowlist anchor missing'; END IF;
  EXECUTE edited;
END $do$;
