SET LOCAL aidash.scope = 'deployment';

-- PostgreSQL function bodies and ALTER TYPE ... USING are untyped DDL.
DO $$
DECLARE original text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 EXECUTE replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_pruning_0090(');
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT NOT (value ?| ARRAY['schema_version','max_steps','tool_parallelism','context_policy','cluster'])
  AND public.aidash_agent_before_pruning_0090(value || '{"schema_version":1}'::jsonb)
$$;

CREATE FUNCTION public.aidash_pruned_definition_0090(entry jsonb) RETURNS jsonb
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE config jsonb;
BEGIN
 IF entry ? 'skills' THEN
  entry := jsonb_set(entry - 'skills', '{tags}', COALESCE((
   SELECT jsonb_agg(DISTINCT tag ORDER BY tag)
   FROM jsonb_array_elements(COALESCE(entry->'tags', '[]'::jsonb) || COALESCE(entry->'skills', '[]'::jsonb)) AS tags(tag)
  ), '[]'::jsonb));
 END IF;

 config := entry->'config';
 IF jsonb_typeof(config) IS DISTINCT FROM 'object' THEN RETURN entry; END IF;
 IF entry->>'kind' = 'agent' THEN
  entry := entry - 'binding_normalization';
  IF jsonb_typeof(config->'bindings') = 'array' THEN
   config := jsonb_set(config, '{bindings}', COALESCE((
    SELECT jsonb_agg(binding ORDER BY ordinal)
    FROM jsonb_array_elements(config->'bindings') WITH ORDINALITY AS bindings(binding, ordinal)
    WHERE binding->>'kind' NOT IN ('memory', 'decider')
   ), '[]'::jsonb));
  END IF;
  config := config - ARRAY['schema_version','max_steps','tool_parallelism','context_policy','cluster',
    'allow_task_creation','allow_task_delegation','allow_memory_write','allow_workspace_retrieval',
    'allow_cross_conversation_memory'];
  IF config->>'harness_contract' = 'legacy@1' THEN config := config - 'harness_contract'; END IF;
  IF config->>'projection_version' IN ('legacy','ordered') THEN config := config - 'projection_version'; END IF;
  IF config #>> '{exposure,version}' = 'legacy@1' THEN config := config - 'exposure'; END IF;
 ELSIF entry->>'kind' = 'model' THEN
  config := config - ARRAY['projection_versions','request_estimator']
   #- '{estimate_diagnostics,calibrations,legacy}' #- '{estimate_diagnostics,calibrations,ordered}';
 END IF;
 RETURN jsonb_set(entry, '{config}', config);
END $$;

ALTER TABLE public.registry ALTER COLUMN metadata TYPE jsonb
 USING public.aidash_pruned_definition_0090(metadata);
ALTER TABLE public.registry_requests ALTER COLUMN request TYPE jsonb
 USING public.aidash_pruned_definition_0090(request);

ALTER TABLE public.agent_drafts ALTER COLUMN entry TYPE jsonb
 USING public.aidash_pruned_definition_0090(entry);

DROP FUNCTION public.aidash_pruned_definition_0090(jsonb);
