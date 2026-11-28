-- PostgreSQL has no typed migration operation for function bodies.
DO $$
DECLARE definition text;
BEGIN
 definition := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 EXECUTE replace(definition, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_goal_0172(');
 definition := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 EXECUTE replace(definition, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_goal_0172(');
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' = 'core.artifacts@1' THEN RETURN false; END IF;
 IF value->>'provider' IS DISTINCT FROM 'core.deliverables@1' THEN
  RETURN public.aidash_descriptor_before_goal_0172(value);
 END IF;
 IF value->>'operation' IS DISTINCT FROM 'deliverable_note'
 OR value->>'default_alias' IS DISTINCT FROM 'deliverable_note' THEN RETURN false; END IF;
 RETURN public.aidash_descriptor_before_goal_0172(value || jsonb_build_object(
  'provider','core.artifacts@1','operation','artifact_publish','default_alias','artifact_publish'));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value ? 'completion' THEN RETURN false; END IF;
 IF jsonb_typeof(value->'remove_default') = 'array' THEN
  value := jsonb_set(value, '{remove_default}', COALESCE((
   SELECT jsonb_agg(CASE WHEN e = '"deliverable_note"'::jsonb
      THEN '"artifact_publish"'::jsonb ELSE e END ORDER BY o)
   FROM jsonb_array_elements(value->'remove_default') WITH ORDINALITY AS r(e, o)), '[]'::jsonb));
 END IF;
 RETURN public.aidash_agent_before_goal_0172(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
