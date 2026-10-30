ALTER TABLE semantic_indexes DROP CONSTRAINT knowledge_embedding_inputs;
DO $$
DECLARE definition text; restored text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 restored := replace(definition, '''dimensions''::text, ''inputs''::text]', '''dimensions''::text]');
 IF restored = definition THEN RAISE EXCEPTION 'embedding allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', restored);
END $$;
