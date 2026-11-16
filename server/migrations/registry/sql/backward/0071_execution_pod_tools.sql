-- Refuse to invalidate retained immutable Execution Pod tool definitions on downgrade.
DO $$
DECLARE original text; restored text;
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' IN ('core.scratch@1', 'core.processes@1', 'core.mcp_stdio@1')) THEN
  RAISE EXCEPTION 'registry 0071_execution_pod_tools cannot be reversed: definitions require Execution Pod tools' USING ERRCODE = '55000';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_before_pod_tools_0071(jsonb)'::regprocedure);
 restored := replace(original, 'FUNCTION public.aidash_descriptor_before_pod_tools_0071(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF restored = original THEN RAISE EXCEPTION 'Descriptor contract backup header missing'; END IF;
 EXECUTE restored;
END $$;
DROP FUNCTION public.aidash_descriptor_before_pod_tools_0071(jsonb);
