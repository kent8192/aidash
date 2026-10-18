-- Restore the 0030_plan_connection_models model constraint.
-- Rollback precondition: no model Definition may carry `request_estimator`.
-- The restored constraint would reject such records, and registered
-- Definitions are immutable, so this read-only guard refuses the rollback
-- instead of leaving rows that the constraint no longer accepts.
DO $$
DECLARE definition_count bigint;
BEGIN
  SELECT count(*) INTO definition_count FROM registry
   WHERE kind = 'model' AND metadata->'config' ? 'request_estimator';
  IF definition_count > 0 THEN
    RAISE EXCEPTION 'registry 0031_model_request_estimator cannot be reversed: % Definitions use request_estimator', definition_count
      USING ERRCODE = '55000',
            HINT = 'The restored constraint rejects these immutable records; delete those model Definitions or restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_model_request_estimator;
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''cache_mode''::text, ''request_estimator''::text]', '''cache_mode''::text]');
  IF edited = definition THEN RAISE EXCEPTION 'Request Estimator rollback allowlist anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
