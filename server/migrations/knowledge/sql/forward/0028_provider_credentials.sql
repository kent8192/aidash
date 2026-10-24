-- Unsupported PostgreSQL JSONB/catalog CHECK DDL only; preserve all current rules.
DO $$
DECLARE definition text; extended text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
  extended := replace(definition, '''credential_env''::text, ''model''::text', '''credential_env''::text, ''provider_credential''::text, ''model''::text');
  IF extended = definition THEN RAISE EXCEPTION 'Semantic Provider Credential allowlist anchor missing'; END IF;
  EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
  EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', extended);
END $$;

ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_provider_credential_config CHECK (
  COALESCE(
    spec #> '{embedding,provider_credential}' IS NULL
    OR spec #> '{embedding,provider_credential}' = 'null'::jsonb
    OR (
      spec #> '{embedding,provider_credential}' = '"openrouter"'::jsonb
      AND spec #>> '{embedding,provider}' = 'openrouter'
      AND spec #>> '{embedding,endpoint}' = 'https://openrouter.ai/api/v1'
      AND (spec #> '{embedding,credential_env}' IS NULL OR spec #> '{embedding,credential_env}' = 'null'::jsonb)
    ), false
  )
);
