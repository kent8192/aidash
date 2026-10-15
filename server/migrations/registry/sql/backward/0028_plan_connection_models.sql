-- Restore the 0020_model_streaming_config model constraint.
-- Rollback precondition: no model Definition may be a Plan Connection model.
-- Earlier binaries decode model configs with deny_unknown_fields and the
-- restored constraint would reject them, so this read-only guard refuses the
-- rollback instead of leaving records no binary can read.
DO $$
DECLARE definition_count bigint;
BEGIN
  SELECT count(*) INTO definition_count FROM registry
   WHERE kind = 'model'
     AND (metadata #>> '{config,provider}' = 'chatgpt' OR metadata->'config' ? 'plan_connection');
  IF definition_count > 0 THEN
    RAISE EXCEPTION 'registry 0028_plan_connection_models cannot be reversed: % Definitions use a Plan Connection', definition_count
      USING ERRCODE = '55000',
            HINT = 'Earlier binaries cannot read these records; delete the chatgpt model Definitions or restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_plan_connection_config;
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''provider_credential''::text, ''plan_connection''::text,', '''provider_credential''::text,');
  IF edited = definition THEN RAISE EXCEPTION 'Plan Connection rollback allowlist anchor missing: registry_model_config'; END IF;
  definition := edited;
  edited := replace(definition, '((metadata #>> ''{config,provider}''::text[]) = ANY (ARRAY[''openrouter''::text, ''chatgpt''::text]))', '((metadata #>> ''{config,provider}''::text[]) = ''openrouter''::text)');
  IF edited = definition THEN RAISE EXCEPTION 'Plan Connection rollback provider anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
