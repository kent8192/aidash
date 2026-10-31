ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_image_limits;
DO $$
DECLARE definition text; restored text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 restored := replace(definition, '''max_input_bytes''::text, ''max_image_bytes''::text, ''max_images''::text]', '''max_input_bytes''::text]');
 IF restored = definition THEN RAISE EXCEPTION 'index limit allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', restored);
END $$;
