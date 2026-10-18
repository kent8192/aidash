-- Request Estimators (#179, #367): admit the model `request_estimator` key.
-- PostgreSQL JSONB CHECK expressions and catalog-driven constraint edits have
-- no typed Reinhardt operation. DDL only; no application data is modified.
-- Extend only the CURRENT model constraint, read from the catalog, so earlier
-- additions (provider_credential, cache_mode, plan_connection) survive.
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''cache_mode''::text]', '''cache_mode''::text, ''request_estimator''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Request Estimator allowlist anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
ALTER TABLE registry ADD CONSTRAINT registry_model_request_estimator CHECK (
  kind <> 'model' OR NOT (metadata->'config' ? 'request_estimator')
  OR metadata #> '{config,request_estimator}' IN ('"bytes"'::jsonb, '"calibrated"'::jsonb)
);
