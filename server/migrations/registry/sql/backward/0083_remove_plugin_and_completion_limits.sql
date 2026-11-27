-- Update the validation body captured from registry/0058 by registry/0061.
DO $$
DECLARE
 definition text := pg_get_functiondef('public.aidash_agent_before_provider_tools_0050(jsonb)'::regprocedure);
 original text := definition;
BEGIN
 IF strpos(original, $before$jsonb_array_length(completion->'checks') < 1$before$) = 0 THEN
  RAISE EXCEPTION 'Agent validation expression missing during registry/0083_remove_plugin_and_completion_limits';
 END IF;
 definition := replace(definition, $before$jsonb_array_length(completion->'checks') < 1$before$, $after$jsonb_array_length(completion->'checks') NOT BETWEEN 1 AND 8$after$);
 IF strpos(original, $before$COALESCE(completion->>'repair_allowance','3')::numeric > 4294967295$before$) = 0 THEN
  RAISE EXCEPTION 'Agent validation expression missing during registry/0083_remove_plugin_and_completion_limits';
 END IF;
 definition := replace(definition, $before$COALESCE(completion->>'repair_allowance','3')::numeric > 4294967295$before$, $after$COALESCE(completion->>'repair_allowance','3')::numeric > 16$after$);
 IF strpos(original, $before$IF jsonb_typeof(plugins) <> 'array' THEN RETURN false; END IF;$before$) = 0 THEN
  RAISE EXCEPTION 'Agent validation expression missing during registry/0083_remove_plugin_and_completion_limits';
 END IF;
 definition := replace(definition, $before$IF jsonb_typeof(plugins) <> 'array' THEN RETURN false; END IF;$before$, $after$IF jsonb_typeof(plugins) <> 'array' OR jsonb_array_length(plugins) > 8 THEN RETURN false; END IF;$after$);
 EXECUTE definition;
END $$;
