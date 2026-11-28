-- A semantic index's Keyless embedding names `openai_compatible` (#1169).
-- PostgreSQL JSONB CHECK expressions edited from the catalog have no typed
-- Reinhardt operation, so this DDL is SQL. It extends only the embedding
-- provider allowlist of the CURRENT constraint. No application data here.
DO $$
DECLARE definition text; edited text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 edited := replace(definition, 'ARRAY[''openai''::text, ''openrouter''::text]', 'ARRAY[''openai''::text, ''openrouter''::text, ''openai_compatible''::text]');
 IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: semantic_indexes_revision'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', edited);
END $$;
