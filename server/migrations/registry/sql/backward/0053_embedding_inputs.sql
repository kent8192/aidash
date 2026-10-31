ALTER TABLE registry DROP CONSTRAINT registry_embedding_inputs;
DO $$
DECLARE definition text; restored text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_embedding_config';
 restored := replace(definition, '''dimensions''::text, ''inputs''::text]', '''dimensions''::text]');
 IF restored = definition THEN RAISE EXCEPTION 'embedding allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE registry DROP CONSTRAINT registry_embedding_config';
 EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_embedding_config %s', restored);
END $$;
