-- Restore the former validation functions only when no stored definition needs the Drive contract.
SET LOCAL aidash.scope = 'deployment';
DO $$
BEGIN
 IF EXISTS (SELECT 1 FROM public.registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.drives@1')
 OR EXISTS (SELECT 1 FROM public.registry WHERE kind = 'agent' AND metadata #> '{config,remove_default}' ? 'drive_search') THEN
  RAISE EXCEPTION 'execution 0179_drive_tool_contract cannot be reversed: definitions use Drive tools'
   USING ERRCODE = '55000';
 END IF;
END $$;

DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_before_drive_0179(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_before_drive_0179(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_before_drive_0179(jsonb);
 original := pg_get_functiondef('public.aidash_agent_before_drive_0179(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_before_drive_0179(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_agent_before_drive_0179(jsonb);
END $$;
