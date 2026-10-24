-- Immutable model definitions are never rewritten to make rollback succeed.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM registry WHERE kind = 'model'
    AND (metadata->'config' ? 'provider_route' OR metadata->'config' ? 'estimate_diagnostics'))
  THEN RAISE EXCEPTION 'Projection measurement rollback is blocked by immutable model definitions'
    USING ERRCODE = '55000', HINT = 'Restore the pre-upgrade database or retain this migration.';
  END IF;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_model_projection_measurement;
DROP FUNCTION public.aidash_estimate_diagnostics_valid(jsonb);
DROP FUNCTION public.aidash_provider_route_valid(jsonb);
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition,
    '''request_estimator''::text, ''provider_route''::text, ''estimate_diagnostics''::text',
    '''request_estimator''::text');
  IF edited = definition THEN RAISE EXCEPTION 'Projection measurement rollback anchor missing'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
