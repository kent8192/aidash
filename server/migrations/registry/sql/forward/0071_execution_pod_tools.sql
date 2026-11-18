-- PostgreSQL validation functions require SQL DDL. Preserve all prior rules.
-- The Execution Pod groups of #745 (file_save, file_remove, file_copy,
-- process_list, process_log, process_stop, mcp_stdio) are Sandboxed operations
-- without a lifecycle, like apply_patch; their providers are fixed.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_before_pod_tools_0071(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'Execution Pod tool descriptor contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_pod_tools_0071(');
 IF copied = original THEN RAISE EXCEPTION 'Descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE expected_provider text;
BEGIN
 expected_provider := CASE value->>'operation'
  WHEN 'file_save' THEN 'core.scratch@1' WHEN 'file_remove' THEN 'core.scratch@1' WHEN 'file_copy' THEN 'core.scratch@1'
  WHEN 'process_list' THEN 'core.processes@1' WHEN 'process_log' THEN 'core.processes@1' WHEN 'process_stop' THEN 'core.processes@1'
  WHEN 'mcp_stdio' THEN 'core.mcp_stdio@1' END;
 IF expected_provider IS NOT NULL THEN
  IF value->>'provider' IS DISTINCT FROM expected_provider
  OR COALESCE(value->'lifecycle','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
  -- Every other rule is apply_patch's: Sandboxed tier, no transport, the same narrowing.
  RETURN public.aidash_descriptor_before_pod_tools_0071(value || '{"provider":"core.files@1","operation":"apply_patch"}'::jsonb);
 END IF;
 RETURN public.aidash_descriptor_before_pod_tools_0071(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
