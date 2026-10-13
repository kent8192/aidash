-- ADR 0006: installation overrides of model and embedding entries never name
-- an environment key either. Earlier versions accepted `credential_env` in a
-- model installation override, and the overlay of such a row now fails
-- ModelConfig validation, so refuse the upgrade while one exists instead of
-- breaking admissions after deployment. Function bodies and data-dependent
-- refusals have no typed Reinhardt operation. DDL only; no data is modified.
DO $$
DECLARE installation_count bigint;
BEGIN
  SELECT count(*) INTO installation_count FROM installations
    JOIN registry ON registry.id = installations.id AND registry.version = installations.version
   WHERE registry.kind IN ('model', 'embedding') AND installations.config ? 'credential_env';
  IF installation_count > 0 THEN
    RAISE EXCEPTION 'registry 0024_model_provider_keys cannot be applied: % installation overrides set credential_env', installation_count
      USING ERRCODE = '55000',
            HINT = 'Remove credential_env from those installation overrides (or reset the development database), then connect OpenRouter for each Tenant through Provider Authorization.';
  END IF;
END $$;
-- Reject `credential_env` in the model section of the CURRENT installation
-- guard, read from the catalog. Only the key's own type clause is replaced, so
-- the allowlists that 0019-0021 edit (and reverse) keep their anchors.
DO $do$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_functiondef('public.guard_installation_config()'::regprocedure) INTO STRICT definition;
  edited := replace(definition, $old$AND (NOT NEW.config ? 'credential_env' OR jsonb_typeof(NEW.config->'credential_env') IN ('string','null'))$old$, $new$AND NOT NEW.config ? 'credential_env'$new$);
  IF edited = definition THEN RAISE EXCEPTION 'installation model override credential_env anchor missing'; END IF;
  EXECUTE edited;
END $do$;
