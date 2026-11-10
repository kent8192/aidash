-- Refuse rollback if a definition or Run snapshot depends on the new default.
DO $$ BEGIN
 IF EXISTS (SELECT 1 FROM public.registry WHERE kind = 'agent'
  AND metadata #> '{config,remove_default}' ? 'connection_request') THEN
  RAISE EXCEPTION 'registry 0065 cannot be reversed: definitions remove connection_request' USING ERRCODE = '55000';
 END IF;
 IF to_regclass('public.runs') IS NOT NULL AND EXISTS (SELECT 1 FROM public.runs WHERE
  jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition.config.remove_default[*] ? (@ == "connection_request")')) THEN
  RAISE EXCEPTION 'registry 0065 cannot be reversed: Runs pin connection_request removal' USING ERRCODE = '55000';
 END IF;
END $$;
-- PostgreSQL function bodies are DDL and have no typed migration operation.
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; restriction jsonb; edges jsonb := '[]'::jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR jsonb_typeof(COALESCE(value->'bindings','[]'::jsonb)) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(COALESCE(value->'bindings','[]'::jsonb)) LOOP
  restriction := COALESCE(item->'narrow','{}'::jsonb);
  IF jsonb_typeof(item) IS DISTINCT FROM 'object' OR jsonb_typeof(restriction) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
  IF COALESCE(restriction->'provider_tool','null'::jsonb) <> 'null'::jsonb
  AND NOT COALESCE(item->>'kind' IN ('tool','bundle') AND public.aidash_provider_tool_binding_is_valid(restriction->'provider_tool'),false) THEN RETURN false; END IF;
  item := jsonb_set(item, '{narrow}', restriction - 'provider_tool');
  edges := edges || jsonb_build_array(item);
 END LOOP;
 RETURN public.aidash_agent_before_provider_tools_0050(jsonb_set(value, '{bindings}', edges));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
