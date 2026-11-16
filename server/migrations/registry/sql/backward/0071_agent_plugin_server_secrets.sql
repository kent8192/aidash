-- PostgreSQL cannot express validation function bodies as typed migration operations. DDL only.
-- Definitions naming a server secret cannot satisfy the earlier validator, and
-- earlier binaries reject Runs whose Binding snapshots pin one.
DO $$ BEGIN
 IF EXISTS (
  SELECT 1 FROM public.registry WHERE kind = 'agent'
  AND jsonb_path_exists(metadata, 'lax $.config.agent_plugin_settings.*.*.secret')
 ) THEN
  RAISE EXCEPTION 'registry 0071 cannot be reversed: Agent definitions name Agent Plugin server secrets'
   USING ERRCODE = '55000',
         HINT = 'Earlier binaries cannot read these definitions; restore the pre-upgrade database backup instead of reversing this migration.';
 END IF;
 -- PL/pgSQL plans a condition whole, so a query naming a missing public.runs
 -- fails even behind a false to_regclass test. The nested IF is planned only
 -- once the table is known to exist.
 IF to_regclass('public.runs') IS NOT NULL THEN
  IF EXISTS(SELECT 1 FROM public.runs
    WHERE jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition ? (@.kind == "agent").config.agent_plugin_settings.*.*.secret')) THEN
   RAISE EXCEPTION 'registry 0071 cannot be reversed: Runs pin Agent definitions naming Agent Plugin server secrets'
    USING ERRCODE = '55000',
          HINT = 'Earlier binaries cannot read these Runs; restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
 END IF;
END $$;
DO $$ DECLARE original text; BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_plugin_secrets_0071(jsonb)'::regprocedure);
 EXECUTE replace(original, 'FUNCTION public.aidash_agent_before_plugin_secrets_0071(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
END $$;
DROP FUNCTION public.aidash_agent_before_plugin_secrets_0071(jsonb);
