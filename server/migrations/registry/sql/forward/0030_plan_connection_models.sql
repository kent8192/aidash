-- Plan Connection models (#159): admit provider `chatgpt` and the optional
-- `plan_connection` key. PostgreSQL JSONB CHECK expressions and catalog-driven
-- constraint edits have no typed Reinhardt operation. DDL only; no
-- application data is modified.
-- Extend only the CURRENT model constraint, read from the catalog, so earlier
-- additions (provider_credential, projection_versions, cache_mode, streaming)
-- survive.
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''provider_credential''::text,', '''provider_credential''::text, ''plan_connection''::text,');
  IF edited = definition THEN RAISE EXCEPTION 'Plan Connection allowlist anchor missing: registry_model_config'; END IF;
  definition := edited;
  edited := replace(definition, '((metadata #>> ''{config,provider}''::text[]) = ''openrouter''::text)', '((metadata #>> ''{config,provider}''::text[]) = ANY (ARRAY[''openrouter''::text, ''chatgpt''::text]))');
  IF edited = definition THEN RAISE EXCEPTION 'Plan Connection provider anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
-- A `chatgpt` model is served only by its caller's Plan Connection, at OpenAI's
-- endpoint, and never by a Provider Credential; no other model names one.
ALTER TABLE registry ADD CONSTRAINT registry_plan_connection_config CHECK (
  kind <> 'model' OR COALESCE(
    CASE
      WHEN metadata #> '{config,plan_connection}' IS NULL OR metadata #> '{config,plan_connection}' = 'null'::jsonb
        THEN metadata #>> '{config,provider}' IS DISTINCT FROM 'chatgpt'
      ELSE metadata #> '{config,plan_connection}' = '"chatgpt"'::jsonb
        AND metadata #>> '{config,provider}' = 'chatgpt'
        AND metadata #>> '{config,endpoint}' = 'https://api.openai.com/v1'
        AND (metadata #> '{config,provider_credential}' IS NULL OR metadata #> '{config,provider_credential}' = 'null'::jsonb)
    END,
    false
  )
);
