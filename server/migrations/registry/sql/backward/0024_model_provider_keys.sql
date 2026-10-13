-- Restore the `credential_env` type clause in the model section of the
-- CURRENT installation guard, returning the 0022_context_policy function body.
-- DDL only.
DO $do$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_functiondef('public.guard_installation_config()'::regprocedure) INTO STRICT definition;
  edited := replace(definition, $old$AND NOT NEW.config ? 'credential_env'$old$, $new$AND (NOT NEW.config ? 'credential_env' OR jsonb_typeof(NEW.config->'credential_env') IN ('string','null'))$new$);
  IF edited = definition THEN RAISE EXCEPTION 'installation model override credential_env rollback anchor missing'; END IF;
  EXECUTE edited;
END $do$;
