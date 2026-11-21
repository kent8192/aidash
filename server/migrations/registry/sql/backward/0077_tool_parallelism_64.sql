-- Validation function DDL; do not invalidate stored Agents.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'agent'
  AND jsonb_typeof(metadata #> '{config,tool_parallelism}') = 'number'
  AND (metadata #>> '{config,tool_parallelism}')::numeric > 16) THEN
  RAISE EXCEPTION 'registry 0077_tool_parallelism_64 cannot be reversed: Agents declare a Tool Parallelism above 16'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_parallelism_0077(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_before_parallelism_0077(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_agent_before_parallelism_0077(jsonb);
END $$;
