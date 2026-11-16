-- PostgreSQL cannot express validation function bodies as typed migration operations. DDL only.
-- A server's `secret` mirrors ServerSecret: an object with a string `name`, an
-- optional string `header` and an optional `owner` of `any` or `personal`.
-- Domain validation checks the name and header rules; the earlier body still
-- checks every other key once `secret` is removed.
DO $$ DECLARE original text; copied text; BEGIN
 IF to_regprocedure('public.aidash_agent_before_plugin_secrets_0071(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'Agent Plugin secret contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_plugin_secrets_0071(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract function header missing'; END IF;
 EXECUTE copied;
END $$;
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE settings jsonb; plugin_name text; plugin_choices jsonb; server_name text; server_choice jsonb; secret jsonb;
BEGIN
 settings := value->'agent_plugin_settings';
 IF jsonb_typeof(settings) = 'object' THEN
  FOR plugin_name, plugin_choices IN SELECT k, v FROM jsonb_each(settings) AS plugin_entries(k,v) LOOP
   CONTINUE WHEN jsonb_typeof(plugin_choices) <> 'object';
   FOR server_name, server_choice IN SELECT k, v FROM jsonb_each(plugin_choices) AS server_entries(k,v) LOOP
    CONTINUE WHEN jsonb_typeof(server_choice) <> 'object' OR NOT server_choice ? 'secret';
    secret := server_choice->'secret';
    IF jsonb_typeof(secret) <> 'object'
    OR secret - ARRAY['name','header','owner']::text[] <> '{}'::jsonb
    OR jsonb_typeof(secret->'name') IS DISTINCT FROM 'string'
    OR (secret ? 'header' AND jsonb_typeof(secret->'header') <> 'string')
    OR (secret ? 'owner' AND NOT secret->'owner' IN ('"any"'::jsonb,'"personal"'::jsonb)) THEN RETURN false; END IF;
    settings := jsonb_set(settings, ARRAY[plugin_name, server_name], server_choice - 'secret');
   END LOOP;
  END LOOP;
  value := jsonb_set(value, '{agent_plugin_settings}', settings);
 END IF;
 RETURN public.aidash_agent_before_plugin_secrets_0071(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
