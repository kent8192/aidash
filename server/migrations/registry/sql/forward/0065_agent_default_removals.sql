-- PostgreSQL function bodies are DDL and have no typed migration operation.
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; restriction jsonb; edges jsonb := '[]'::jsonb; removals jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR jsonb_typeof(COALESCE(value->'bindings','[]'::jsonb)) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 removals := COALESCE(value->'remove_default','[]'::jsonb);
 IF jsonb_typeof(removals) IS DISTINCT FROM 'array'
 OR removals ? 'compaction'
 OR (SELECT count(*) <> count(DISTINCT element) FROM jsonb_array_elements(removals) AS items(element)) THEN RETURN false; END IF;
 -- The previous validator does not know connection_request; remove it only
 -- for delegation after checking uniqueness and preserving every other rule.
 value := jsonb_set(value, '{remove_default}', removals - 'connection_request', true);
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
