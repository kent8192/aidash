-- Keyless model and embedding definitions name `openai_compatible` (#1169).
-- PostgreSQL JSONB CHECK expressions edited from the catalog have no typed
-- Reinhardt operation, so this DDL is SQL. It extends only the provider
-- allowlists of the CURRENT constraints, so earlier additions survive. No
-- application data is modified here.
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, 'ARRAY[''openrouter''::text, ''chatgpt''::text]', 'ARRAY[''openrouter''::text, ''chatgpt''::text, ''openai_compatible''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_embedding_config';
  edited := replace(definition, 'ARRAY[''openai''::text, ''openrouter''::text]', 'ARRAY[''openai''::text, ''openrouter''::text, ''openai_compatible''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: registry_embedding_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_embedding_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_embedding_config %s', edited);
END $$;
