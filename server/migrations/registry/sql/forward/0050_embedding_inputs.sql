-- PostgreSQL catalog-based CHECK DDL is not expressible by Reinhardt SchemaExpr.
DO $$
DECLARE definition text; extended text;
BEGIN
 SELECT pg_get_constraintdef(oid) INTO STRICT definition FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_embedding_config';
 extended := replace(definition, '''dimensions''::text]', '''dimensions''::text, ''inputs''::text]');
 IF extended = definition THEN RAISE EXCEPTION 'embedding allowlist anchor missing'; END IF;
 EXECUTE 'ALTER TABLE registry DROP CONSTRAINT registry_embedding_config';
 EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_embedding_config %s', extended);
END $$;
ALTER TABLE registry ADD CONSTRAINT registry_embedding_inputs CHECK (kind <> 'embedding' OR (metadata #> '{config,inputs}' IS NULL OR metadata #> '{config,inputs}' IN ('["text"]'::jsonb, '["text","image"]'::jsonb, '["image","text"]'::jsonb)));
