-- Restore the embedding provider allowlist of 0024 after the data reverse put
-- every Keyless index embedding back on `openai`.
DO $$
DECLARE definition text; edited text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 edited := replace(definition, 'ARRAY[''openai''::text, ''openrouter''::text, ''openai_compatible''::text]', 'ARRAY[''openai''::text, ''openrouter''::text]');
 IF edited = definition THEN RAISE EXCEPTION 'Keyless provider anchor missing: semantic_indexes_revision'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', edited);
END $$;
