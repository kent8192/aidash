-- PostgreSQL catalog-based CHECK DDL is not expressible by Reinhardt SchemaExpr.
DO $$
DECLARE definition text; extended text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
 extended := replace(definition, '''dimensions''::text]', '''dimensions''::text, ''inputs''::text]');
 IF extended = definition THEN RAISE EXCEPTION 'embedding allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
 EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', extended);
END $$;
ALTER TABLE semantic_indexes ADD CONSTRAINT knowledge_embedding_inputs CHECK ((spec #> '{embedding,inputs}' IS NULL OR spec #> '{embedding,inputs}' IN ('["text"]'::jsonb, '["text","image"]'::jsonb, '["image","text"]'::jsonb)));
