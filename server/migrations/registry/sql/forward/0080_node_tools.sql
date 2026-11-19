-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the core.nodes@1 builtins (#1012) and the core.duties@1 builtin
-- duty_notify (#1013). Their descriptors have the shape of the Duty tools
-- (provider, operation, default alias, builtin tier, default narrowing), so
-- they are judged by those rules; retain the previous validator.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_nodes_0080(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'node tool contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_nodes_0080(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' = 'core.duties@1' AND value->>'operation' = 'duty_notify' THEN
  IF value->>'default_alias' IS DISTINCT FROM 'duty_notify' THEN RETURN false; END IF;
  RETURN public.aidash_descriptor_is_valid_before_nodes_0080(value || jsonb_build_object(
   'operation', 'duty_create', 'default_alias', 'duty_create'));
 END IF;
 IF value->>'provider' IS DISTINCT FROM 'core.nodes@1' THEN RETURN public.aidash_descriptor_is_valid_before_nodes_0080(value); END IF;
 IF value->>'operation' IS NULL OR value->>'operation' NOT IN ('node_create','node_delete')
  OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 RETURN public.aidash_descriptor_is_valid_before_nodes_0080(value || jsonb_build_object(
  'provider', 'core.duties@1',
  'operation', 'duty_create',
  'default_alias', 'duty_create'));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
