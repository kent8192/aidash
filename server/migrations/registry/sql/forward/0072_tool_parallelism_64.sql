-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit a Tool Parallelism of 17 to 64 (#1009) by judging such a value as 16
-- with the previous validator; nothing else is relaxed.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_agent_before_parallelism_0072(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'tool parallelism contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_parallelism_0072(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF jsonb_typeof(value->'tool_parallelism') = 'number'
 AND COALESCE(value->>'tool_parallelism' ~ '^[0-9]+$', false)
 AND (value->>'tool_parallelism')::numeric BETWEEN 17 AND 64 THEN
  value := jsonb_set(value, '{tool_parallelism}', '16'::jsonb);
 END IF;
 RETURN public.aidash_agent_before_parallelism_0072(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
