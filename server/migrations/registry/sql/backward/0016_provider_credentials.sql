-- Reverse only the Provider Credential additions; retain the current complete
-- model and embedding constraints rather than restoring an older snapshot.
ALTER TABLE registry DROP CONSTRAINT registry_provider_credential_config;
DO $$
DECLARE constraint_name text; definition text; previous text;
BEGIN
  FOREACH constraint_name IN ARRAY ARRAY['registry_model_config', 'registry_embedding_config'] LOOP
    SELECT pg_get_constraintdef(oid) INTO STRICT definition
      FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = constraint_name;
    previous := replace(definition, '''provider_credential''::text, ', '');
    IF previous = definition THEN RAISE EXCEPTION 'Provider Credential allowlist addition missing: %', constraint_name; END IF;
    EXECUTE format('ALTER TABLE registry DROP CONSTRAINT %I', constraint_name);
    EXECUTE format('ALTER TABLE registry ADD CONSTRAINT %I %s', constraint_name, previous);
  END LOOP;
END $$;
