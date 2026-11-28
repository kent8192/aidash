-- Restore the provider allowlists of 0030 and 0014. Runs after the data
-- reverse of 0083 put every Keyless definition back on `openrouter` (models)
-- or `openai` (embeddings).
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, 'ARRAY[''openrouter''::text, ''chatgpt''::text, ''openai_compatible''::text]', 'ARRAY[''openrouter''::text, ''chatgpt''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_embedding_config';
  edited := replace(definition, 'ARRAY[''openai''::text, ''openrouter''::text, ''openai_compatible''::text]', 'ARRAY[''openai''::text, ''openrouter''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: registry_embedding_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_embedding_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_embedding_config %s', edited);
END $$;
