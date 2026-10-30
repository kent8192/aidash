-- Preserve all existing constraints while extending the outer IndexingSpec allowlist.
DO $$
DECLARE definition text; extended text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 extended := replace(definition, '''max_input_bytes''::text]', '''max_input_bytes''::text, ''max_image_bytes''::text, ''max_images''::text]');
 IF extended = definition THEN RAISE EXCEPTION 'index limit allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', extended);
END $$;
ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_image_limits CHECK (
  (CASE WHEN spec->'max_image_bytes' IS NULL THEN true
   WHEN jsonb_typeof(spec->'max_image_bytes') = 'number' AND (spec->'max_image_bytes')::text ~ '^(0|[1-9][0-9]*)$'
   THEN (spec->>'max_image_bytes')::numeric BETWEEN 1 AND 67108864 ELSE false END)
  AND (CASE WHEN spec->'max_images' IS NULL THEN true
   WHEN jsonb_typeof(spec->'max_images') = 'number' AND (spec->'max_images')::text ~ '^(0|[1-9][0-9]*)$'
   THEN (spec->>'max_images')::numeric BETWEEN 0 AND 1024 ELSE false END)
);
