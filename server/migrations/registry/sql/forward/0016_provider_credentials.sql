-- PostgreSQL JSONB CHECK expressions and catalog-driven DDL are unsupported by
-- Reinhardt SchemaExpr. Preserve each CURRENT constraint, including the #133
-- merge, and extend only its config allowlist. No application data SQL.
DO $$
DECLARE constraint_name text; definition text; extended text;
BEGIN
  FOREACH constraint_name IN ARRAY ARRAY['registry_model_config', 'registry_embedding_config'] LOOP
    SELECT pg_get_constraintdef(oid) INTO STRICT definition
      FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = constraint_name;
    extended := replace(definition, '''credential_env''::text,', '''credential_env''::text, ''provider_credential''::text,');
    IF extended = definition THEN RAISE EXCEPTION 'Provider Credential allowlist anchor missing: %', constraint_name; END IF;
    EXECUTE format('ALTER TABLE registry DROP CONSTRAINT %I', constraint_name);
    EXECUTE format('ALTER TABLE registry ADD CONSTRAINT %I %s', constraint_name, extended);
  END LOOP;
END $$;

ALTER TABLE registry ADD CONSTRAINT registry_provider_credential_config CHECK (
  kind NOT IN ('model', 'embedding') OR COALESCE(
    metadata #> '{config,provider_credential}' IS NULL
    OR metadata #> '{config,provider_credential}' = 'null'::jsonb
    OR (
      metadata #> '{config,provider_credential}' = '"openrouter"'::jsonb
      AND metadata #>> '{config,provider}' = 'openrouter'
      AND metadata #>> '{config,endpoint}' = 'https://openrouter.ai/api/v1'
      AND (metadata #> '{config,credential_env}' IS NULL OR metadata #> '{config,credential_env}' = 'null'::jsonb)
    ), false
  )
);
