-- Restore the previous allowlist while preserving independent current rules.
ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_provider_credential_config;
DO $$
DECLARE definition text; restored text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'semantic_indexes'::regclass AND conname = 'semantic_indexes_revision';
  restored := replace(definition, '''provider_credential''::text, ', '');
  IF restored = definition THEN RAISE EXCEPTION 'Semantic Provider Credential rollback anchor missing'; END IF;
  EXECUTE 'ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision';
  EXECUTE format('ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision %s', restored);
END $$;
